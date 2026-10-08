//! `debug.getinfo`: option checking and the result table.

use super::{check_int, getthread, is_function, set_field, set_str};
use crate::runtime::{Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, opt_string, to_num};
use crate::vm::builtins::arg_error;
use crate::vm::callstack::{Ar, DbgKind};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// The option letters `lua_getinfo` accepts in each version.
fn valid_options(v: LuaVersion) -> &'static [u8] {
    match v {
        LuaVersion::Lua51 => b"SlunLf",
        LuaVersion::Lua52 | LuaVersion::Lua53 => b"SlutnLf",
        _ => b"SlutnrLf",
    }
}

pub(super) fn d_getinfo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let (co, arg) = getthread(vm, a);
    let options: Vec<u8> = match opt_string(vm, a, arg + 1)? {
        Some(s) => s.as_bytes().to_vec(),
        None => match v {
            LuaVersion::Lua51 => b"flnSu".to_vec(),
            LuaVersion::Lua52 | LuaVersion::Lua53 => b"flnStu".to_vec(),
            _ => b"flnSrtu".to_vec(),
        },
    };
    if v >= LuaVersion::Lua54 && options.first() == Some(&b'>') {
        return Err(arg_error(vm, arg + 2, "invalid option '>'"));
    }
    let subject = a.get(vm, arg);
    let level = if v <= LuaVersion::Lua52 {
        // `lua_isnumber` comes first, so a numeric string is a level
        match to_num(vm, subject) {
            Some(n) if !a.is_none(arg) => Some(match n {
                crate::numeric::Num::Int(i) => i,
                crate::numeric::Num::Float(f) => f as i64,
            } as i32 as i64),
            _ if is_function(vm, a, arg) => None,
            _ => return Err(arg_error(vm, arg + 1, "function or level expected")),
        }
    } else if is_function(vm, a, arg) {
        None
    } else {
        Some(check_int(vm, a, arg)?)
    };
    let (ar, activelines) = match level {
        None => {
            let ar = vm.function_ar(subject);
            let lines = activelines(vm, subject);
            (ar, lines)
        }
        Some(level) => {
            let ts = vm.thread_stack(co);
            let found = if level < 0 {
                // 5.1's `lua_getstack` reports a negative level as a lost
                // tail call
                (v == LuaVersion::Lua51).then_some(None)
            } else {
                ts.levels
                    .get(level as usize)
                    .map(|&k| Some((level as usize, k)))
            };
            match found {
                None => {
                    drop(ts);
                    return Ok(vm.nat_return(fs, &[Value::Nil]));
                }
                Some(None) | Some(Some((_, DbgKind::Tail))) => {
                    // PUC 5.1 `auxgetinfo` returns before looking at the
                    // options of a lost tail call
                    drop(ts);
                    let ar = crate::vm::callstack::tail_ar();
                    let t = info_table(vm, &options, &ar, Value::Nil);
                    return Ok(vm.nat_return(fs, &[Value::Table(t)]));
                }
                Some(Some((i, _))) => {
                    let ar = vm.level_ar(&ts, i);
                    drop(ts);
                    let lines = activelines(vm, ar.func);
                    (ar, lines)
                }
            }
        }
    };
    // ≤5.3 do not refuse the '>' that asks for the function on top of the
    // stack: with a level, `lua_getinfo` pops the last argument as that
    // function and describes it as a C function
    let popped = v <= LuaVersion::Lua53 && level.is_some() && options.first() == Some(&b'>');
    let (ar, activelines, checked) = if popped {
        vm.native_pop(1);
        let f = a.get(vm, nargs - 1);
        (popped_ar(f), Value::Nil, &options[1..])
    } else {
        (ar, activelines, &options[..])
    };
    let allowed = valid_options(v);
    if checked.iter().any(|c| !allowed.contains(c)) {
        return Err(arg_error(vm, arg + 2, "invalid option"));
    }
    let t = info_table(vm, &options, &ar, activelines);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

/// PUC `collectvalidlines`: the lines holding an instruction, as a set; nil
/// for a C function.
pub(crate) fn activelines(vm: &mut Vm, f: Value) -> Value {
    let Value::Closure(cl) = f else {
        return Value::Nil;
    };
    let lines = vm.heap.new_table();
    for &ln in cl.proto.lines.iter() {
        // SAFETY: `lines` was allocated above and is held only by this local; each borrow lives for one `set`, which does not collect, and the loop reads `cl.proto`, a different object
        unsafe { lines.as_mut() }
            .set(&mut vm.heap, Value::Int(ln as i64), Value::Bool(true))
            .expect("valid line key");
    }
    vm.barrier_back_table(lines);
    Value::Table(lines)
}

/// `db_getinfo`'s result table: the fields of each requested option.
fn info_table(vm: &mut Vm, options: &[u8], ar: &Ar, activelines: Value) -> Gc<Table> {
    let v = vm.version();
    let t = vm.heap.new_table();
    let has = |c: u8| options.contains(&c);
    if has(b'S') {
        // ≤5.3 push the source as a C string (up to a NUL); 5.4+ keep its
        // length. `short_src` is always a C string.
        let source = if v <= LuaVersion::Lua53 {
            cstr(&ar.source)
        } else {
            &ar.source[..]
        };
        set_str(vm, t, "source", source);
        set_str(vm, t, "short_src", cstr(&ar.short_src));
        set_field(vm, t, "linedefined", Value::Int(ar.linedefined));
        set_field(vm, t, "lastlinedefined", Value::Int(ar.lastlinedefined));
        set_str(vm, t, "what", ar.what.as_bytes());
    }
    if has(b'l') {
        set_field(vm, t, "currentline", Value::Int(ar.currentline));
    }
    if has(b'u') {
        set_field(vm, t, "nups", Value::Int(ar.nups));
        if v >= LuaVersion::Lua52 {
            set_field(vm, t, "nparams", Value::Int(ar.nparams));
            set_field(vm, t, "isvararg", Value::Bool(ar.isvararg));
        }
    }
    if has(b'n') {
        match &ar.name {
            Some((what, name)) => {
                set_str(vm, t, "name", name.as_bytes());
                set_str(vm, t, "namewhat", what.as_bytes());
            }
            None => set_str(vm, t, "namewhat", b""),
        }
    }
    if has(b'r') {
        set_field(vm, t, "ftransfer", Value::Int(ar.ftransfer));
        set_field(vm, t, "ntransfer", Value::Int(ar.ntransfer));
    }
    if has(b't') {
        set_field(vm, t, "istailcall", Value::Bool(ar.istailcall));
        if v >= LuaVersion::Lua55 {
            set_field(vm, t, "extraargs", Value::Int(ar.extraargs));
        }
    }
    if has(b'L') {
        set_field(vm, t, "activelines", activelines);
    }
    if has(b'f') {
        set_field(vm, t, "func", ar.func);
    }
    vm.barrier_back_table(t);
    t
}

fn cstr(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(n) => &b[..n],
        None => b,
    }
}

/// What ≤5.3's `lua_getinfo` reports for a value that is not a Lua
/// function: a C function with no upvalues.
fn popped_ar(f: Value) -> crate::vm::callstack::Ar {
    let mut ar = crate::vm::callstack::Ar {
        nups: 0,
        func: f,
        ..Default::default()
    };
    ar.what = "C";
    ar.source = b"=[C]".to_vec();
    ar.short_src = b"[C]".to_vec();
    ar.linedefined = -1;
    ar.lastlinedefined = -1;
    ar.currentline = -1;
    ar.isvararg = true;
    ar
}
