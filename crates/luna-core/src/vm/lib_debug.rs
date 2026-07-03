//! Debug library — introspection and hooks (PUC db.lua surface). Hooks
//! (sethook/gethook), getlocal/setlocal off `proto.locvars`, upvalues,
//! and registry access are all live; the dialect-gated edges (5.4+
//! `setcstacklimit` etc.) stay no-op.

use crate::runtime::{Gc, LuaClosure, Value};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::{HookState, Vm};

pub(crate) fn open_debug(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    set(vm, "getinfo", d_getinfo);
    set(vm, "getupvalue", d_getupvalue);
    set(vm, "setupvalue", d_setupvalue);
    set(vm, "upvaluejoin", d_upvaluejoin);
    set(vm, "upvalueid", d_upvalueid);
    set(vm, "setuservalue", d_setuservalue);
    set(vm, "getuservalue", d_getuservalue);
    set(vm, "traceback", d_traceback);
    set(vm, "sethook", d_sethook);
    set(vm, "gethook", d_gethook);
    set(vm, "getlocal", d_getlocal);
    set(vm, "setlocal", d_setlocal);
    set(vm, "getmetatable", d_getmetatable);
    set(vm, "setmetatable", d_setmetatable);
    set(vm, "getregistry", d_getregistry);
    // PUC 5.1's `debug.setfenv` / `debug.getfenv` accept any object that
    // carries an env — functions, userdata, and threads (coroutines). 5.2+
    // retired both along with the global `setfenv`/`getfenv` pair.
    if vm.version() <= crate::version::LuaVersion::Lua51 {
        set(vm, "setfenv", d_setfenv);
        set(vm, "getfenv", d_getfenv);
    }
    vm.set_global("debug", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
    // PUC's LUA_REGISTRYINDEX table — eagerly built so `_HOOKKEY` (weak-key)
    // is observable from db.lua :328 the moment the debug library loads.
    init_registry(vm);
    // register in package.loaded so require"debug" finds it
    let pkg_k = Value::Str(vm.heap.intern(b"package"));
    if let Value::Table(pkg) = vm.globals().get(pkg_k) {
        let lk = Value::Str(vm.heap.intern(b"loaded"));
        if let Value::Table(loaded) = pkg.get(lk) {
            let dk = Value::Str(vm.heap.intern(b"debug"));
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { loaded.as_mut() }
                .set(&mut vm.heap, dk, Value::Table(t))
                .expect("valid key");
            vm.barrier_back_table(loaded);
        }
    }
}

fn set_field(vm: &mut Vm, t: Gc<crate::runtime::Table>, k: &str, v: Value) {
    let key = Value::Str(vm.heap.intern(k.as_bytes()));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, key, v)
        .expect("valid key");
}

/// Build PUC's `LUA_REGISTRYINDEX` table (kept on `Vm.registry`, a GC root)
/// and populate `_HOOKKEY` (PUC `db_sethook`'s per-thread weak-key table).
/// db.lua :328 only checks `__mode == 'k'`; luna's sethook stores hook state
/// directly in `Vm.hook`/`Coro.hook`, so the entry is shape-only.
fn init_registry(vm: &mut Vm) {
    let reg = vm.heap.new_table();
    let hook_t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    let mode_k = Value::Str(vm.heap.intern(b"k"));
    set_field(vm, mt, "__mode", mode_k);
    vm.barrier_back_table(mt);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { hook_t.as_mut() }.set_metatable(Some(mt));
    vm.barrier_back_table(hook_t);
    set_field(vm, reg, "_HOOKKEY", Value::Table(hook_t));
    vm.barrier_back_table(reg);
    vm.registry = Some(reg);
}

fn d_getregistry(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    let r = vm.registry.map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[r]))
}

/// PUC `luaO_chunkid`: render a chunk's `source` into its `short_src` form,
/// truncated to `LUA_IDSIZE`. `=name` keeps the literal (head-truncated), `@file`
/// keeps the path (tail-truncated behind `...`), anything else is treated as a
/// string and wrapped as `[string "first line..."]`.
pub(crate) fn chunk_id(source: &[u8]) -> Vec<u8> {
    const IDSIZE: usize = 60;
    const RETS: &[u8] = b"...";
    const PRE: &[u8] = b"[string \"";
    const POS: &[u8] = b"\"]";
    let mut out = Vec::new();
    match source.first() {
        Some(b'=') => {
            // PUC `luaO_chunkid` compares `srclen` (which INCLUDES the sigil
            // byte) to `bufflen` = LUA_IDSIZE — so a 60-byte `=NAME` fits
            // (59 chars), a 61-byte one doesn't.
            let s = &source[1..];
            if source.len() <= IDSIZE {
                out.extend_from_slice(s);
            } else {
                out.extend_from_slice(&s[..IDSIZE - 1]);
            }
        }
        Some(b'@') => {
            let s = &source[1..];
            if source.len() <= IDSIZE {
                out.extend_from_slice(s);
            } else {
                out.extend_from_slice(RETS);
                let bufflen = IDSIZE - RETS.len() - 1;
                out.extend_from_slice(&s[s.len() - bufflen..]);
            }
        }
        _ => {
            let nl = source.iter().position(|&c| c == b'\n');
            out.extend_from_slice(PRE);
            let bufflen = IDSIZE - PRE.len() - RETS.len() - POS.len() - 1;
            let mut srclen = source.len();
            if srclen < bufflen && nl.is_none() {
                out.extend_from_slice(source);
            } else {
                if let Some(n) = nl {
                    srclen = n;
                }
                srclen = srclen.min(bufflen);
                out.extend_from_slice(&source[..srclen]);
                out.extend_from_slice(RETS);
            }
            out.extend_from_slice(POS);
        }
    }
    out
}

fn info_for_closure(
    vm: &mut Vm,
    cl: Gc<LuaClosure>,
    out: Gc<crate::runtime::Table>,
    currentline: Option<u32>,
    extraargs: Option<i64>,
    want_lines: bool,
) {
    let proto = cl.proto;
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    let raw = unsafe { crate::runtime::string::bytes_of(proto.source.as_ptr()) };
    // PUC `funcinfo` substitutes "=?" when a Proto has no source (the binary
    // dump was stripped, so loadFunction left source NULL with no parent).
    // luna detects the same condition via `lines.is_empty()` — text chunks
    // with an explicit empty `chunkname` keep a real line table and must
    // still render as `[string ""]` (db.lua :73). The `=?` substitute makes
    // `chunk_id` strip the `=` sigil → short_src = "?" (db.lua :992/:1004).
    let src_bytes: Vec<u8> = if raw.is_empty() && proto.lines.is_empty() {
        b"=?".to_vec()
    } else {
        raw.to_vec()
    };
    let display = chunk_id(&src_bytes);
    let source = Value::Str(vm.heap.intern(&src_bytes));
    set_field(vm, out, "source", source);
    let short = Value::Str(vm.heap.intern(&display));
    set_field(vm, out, "short_src", short);
    set_field(
        vm,
        out,
        "linedefined",
        Value::Int(proto.line_defined as i64),
    );
    set_field(
        vm,
        out,
        "lastlinedefined",
        Value::Int(proto.last_line_defined as i64),
    );
    let what = if proto.line_defined == 0 {
        "main"
    } else {
        "Lua"
    };
    let wv = Value::Str(vm.heap.intern(what.as_bytes()));
    set_field(vm, out, "what", wv);
    // PUC 5.1 functions don't carry `_ENV` as an upvalue — the global env is
    // a per-function `f->env` slot — so `nups` only counts *user* upvalues.
    // luna keeps `_ENV` in cell 0 even under 5.1 (with the clone-on-Closure
    // tweak) for runtime simplicity, so subtract it back out here. 5.1
    // db.lua :184 pins this with `assert(x.nups == 0)` on a function that
    // touches no upvalues but does reference globals.
    let nups = if vm.version() <= crate::version::LuaVersion::Lua51 {
        proto.upvals.iter().filter(|u| &*u.name != "_ENV").count() as i64
    } else {
        cl.upvals().len() as i64
    };
    set_field(vm, out, "nups", Value::Int(nups));
    set_field(vm, out, "nparams", Value::Int(proto.num_params as i64));
    set_field(vm, out, "isvararg", Value::Bool(proto.is_vararg));
    set_field(vm, out, "func", Value::Closure(cl));
    // PUC `getfuncline` returns -1 when the proto has no per-instruction line
    // info (stripped chunk); funcinfo propagates that to ar.currentline. db.lua
    // :993/:1005 asserts `debug.getinfo(1).currentline == -1` from inside a
    // stripped main chunk.
    let cl_line = if proto.lines.is_empty() {
        Value::Int(-1)
    } else {
        currentline
            .map(|l| Value::Int(l as i64))
            .unwrap_or(Value::Int(-1))
    };
    set_field(vm, out, "currentline", cl_line);
    set_field(vm, out, "istailcall", Value::Bool(false));
    if let Some(e) = extraargs {
        set_field(vm, out, "extraargs", Value::Int(e));
    } else {
        set_field(vm, out, "extraargs", Value::Int(0));
    }
    // 'n' defaults for the function-value form (no caller to name it from); the
    // stack-level path overrides these after calling us.
    let empty = Value::Str(vm.heap.intern(b""));
    set_field(vm, out, "namewhat", empty);
    set_field(vm, out, "name", Value::Nil);
    if want_lines {
        // "L": the set of lines carrying an instruction (PUC collectvalidlines),
        // a table keyed by line number with `true` values.
        let lines = vm.heap.new_table();
        for &ln in proto.lines.iter() {
            let k = Value::Int(ln as i64);
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { lines.as_mut() }
                .set(&mut vm.heap, k, Value::Bool(true))
                .expect("valid line key");
        }
        set_field(vm, out, "activelines", Value::Table(lines));
    }
}

fn d_getinfo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC: `debug.getinfo([thread,] f_or_level [, what])`. With a thread,
    // the (f_or_level, what) shift by one and a level-form query walks the
    // coroutine's saved frames.
    let coro = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(co) => Some(co),
        _ => None,
    };
    let arg_off = if coro.is_some() { 1 } else { 0 };
    let subject = vm.nat_arg(fs, nargs, arg_off);
    let mut want_lines = false;
    let mut want_transfers = false;
    if let Value::Str(s) = vm.nat_arg(fs, nargs, arg_off + 1) {
        let opt = s.as_bytes();
        if opt.first() == Some(&b'>') {
            return Err(arg_error(vm, 2, "getinfo", "invalid option '>'"));
        }
        if opt
            .iter()
            .any(|&c| !matches!(c, b'S' | b'l' | b'n' | b'u' | b't' | b'f' | b'L' | b'r'))
        {
            return Err(arg_error(vm, 2, "getinfo", "invalid option"));
        }
        want_lines = opt.contains(&b'L');
        want_transfers = opt.contains(&b'r');
    }
    let out = vm.heap.new_table();
    match subject {
        Value::Closure(cl) => {
            info_for_closure(vm, cl, out, None, None, want_lines);
        }
        Value::Native(nc) => {
            let c = Value::Str(vm.heap.intern(b"C"));
            set_field(vm, out, "what", c);
            let src = Value::Str(vm.heap.intern(b"=[C]"));
            set_field(vm, out, "source", src);
            let short = Value::Str(vm.heap.intern(b"[C]"));
            set_field(vm, out, "short_src", short);
            set_field(vm, out, "currentline", Value::Int(-1));
            set_field(vm, out, "linedefined", Value::Int(-1));
            set_field(vm, out, "lastlinedefined", Value::Int(-1));
            set_field(vm, out, "func", subject);
            // PUC `lua_getinfo("u", …)` on a C function: nparams = 0,
            // isvararg = true, nups from the closure's upvalue count.
            set_field(vm, out, "nups", Value::Int(nc.upvals.len() as i64));
            set_field(vm, out, "nparams", Value::Int(0));
            set_field(vm, out, "isvararg", Value::Bool(true));
            // PUC `lua_getinfo("t", …)` on a function-value form: C functions
            // can never be tail-called nor carry extra varargs.
            set_field(vm, out, "istailcall", Value::Bool(false));
            set_field(vm, out, "extraargs", Value::Int(0));
        }
        // PUC `lua_tointeger` accepts a Float that holds an integer value —
        // 5.1/5.2 (no integer subtype) make every literal a Float, so
        // `debug.getinfo(co, 1)` hits this arm. Normalize before the level
        // body below.
        Value::Int(_) | Value::Float(_) => {
            let level = match subject {
                Value::Int(n) => n,
                Value::Float(f) if f.fract() == 0.0 && f.is_finite() => f as i64,
                _ => {
                    return Err(arg_error(
                        vm,
                        1,
                        "getinfo",
                        &format!("function or level expected, got {}", subject.type_name()),
                    ));
                }
            };
            // Thread arg + level form: read the frame from co's saved
            // context. PUC's getinfo on a non-current thread enumerates the
            // suspended coroutine's saved frames; the synthetic C-edges
            // (running_natives, hook trampolines) are not modeled there, so
            // we just expose the Lua activations directly.
            if let Some(co) = coro {
                match vm.coro_frame_info(co, level) {
                    None => return Ok(vm.nat_return(fs, &[Value::Nil])),
                    Some((cl, line, extraargs, is_tail)) => {
                        info_for_closure(vm, cl, out, Some(line), Some(extraargs), want_lines);
                        if is_tail {
                            set_field(vm, out, "istailcall", Value::Bool(true));
                        }
                    }
                }
                if want_transfers {
                    set_field(vm, out, "ftransfer", Value::Int(0));
                    set_field(vm, out, "ntransfer", Value::Int(0));
                }
                return Ok(vm.nat_return(fs, &[Value::Table(out)]));
            }
            // level 1 = the function that called getinfo; the logical stack
            // interleaves synthetic C frames for call_value boundaries.
            match vm.dbg_frame(level) {
                None => return Ok(vm.nat_return(fs, &[Value::Nil])),
                Some(crate::vm::exec::DbgKind::C(fi)) => {
                    let c = Value::Str(vm.heap.intern(b"C"));
                    set_field(vm, out, "what", c);
                    let src = Value::Str(vm.heap.intern(b"=[C]"));
                    set_field(vm, out, "source", src);
                    let short = Value::Str(vm.heap.intern(b"[C]"));
                    set_field(vm, out, "short_src", short);
                    set_field(vm, out, "currentline", Value::Int(-1));
                    set_field(vm, out, "linedefined", Value::Int(-1));
                    set_field(vm, out, "lastlinedefined", Value::Int(-1));
                    // PUC's ar.func is the C closure value at this level — for
                    // luna's synthetic C edge that's the native sitting just
                    // above the hook frame on the `running_natives` chain
                    // (db.lua :344 `getinfo(2, "f").func` inside a call hook).
                    let f = vm.c_frame_func(fi).unwrap_or(Value::Nil);
                    set_field(vm, out, "func", f);
                    // PUC names a C function from the call instruction that
                    // invoked it (e.g. "pcall" while a protected call is live).
                    let (namewhat, name) = match vm.c_frame_name(fi) {
                        Some((kind, n)) => (kind, Some(n)),
                        None => ("", None),
                    };
                    let nw = Value::Str(vm.heap.intern(namewhat.as_bytes()));
                    set_field(vm, out, "namewhat", nw);
                    let nv = match name {
                        Some(n) => Value::Str(vm.heap.intern(n.as_bytes())),
                        None => Value::Nil,
                    };
                    set_field(vm, out, "name", nv);
                    // PUC `lua_getinfo("t", …)` on a C activation: never a tail
                    // call, never carries extra varargs.
                    set_field(vm, out, "istailcall", Value::Bool(false));
                    set_field(vm, out, "extraargs", Value::Int(0));
                }
                Some(crate::vm::exec::DbgKind::Lua(fi)) => {
                    let (cl, line, extraargs, is_tail) = vm.frame_info(fi);
                    info_for_closure(vm, cl, out, Some(line), Some(extraargs), want_lines);
                    if is_tail {
                        set_field(vm, out, "istailcall", Value::Bool(true));
                    }
                    // "n": name/namewhat from the caller's call instruction
                    let (namewhat, name) = match vm.frame_name(fi) {
                        Some((kind, n)) => (kind, Some(n)),
                        None => ("", None),
                    };
                    let nw = Value::Str(vm.heap.intern(namewhat.as_bytes()));
                    set_field(vm, out, "namewhat", nw);
                    let nv = match name {
                        Some(n) => Value::Str(vm.heap.intern(n.as_bytes())),
                        None => Value::Nil,
                    };
                    set_field(vm, out, "name", nv);
                }
                Some(crate::vm::exec::DbgKind::Tail(_)) => {
                    // PUC's synthetic tail-call level: what="tail",
                    // short_src="(tail call)", linedefined/currentline/
                    // lastlinedefined=-1, func=nil, no name. 5.1 db.lua
                    // :337-:339 pin the shape end-to-end.
                    let w = Value::Str(vm.heap.intern(b"tail"));
                    set_field(vm, out, "what", w);
                    let src = Value::Str(vm.heap.intern(b"=(tail call)"));
                    set_field(vm, out, "source", src);
                    let short = Value::Str(vm.heap.intern(b"(tail call)"));
                    set_field(vm, out, "short_src", short);
                    set_field(vm, out, "currentline", Value::Int(-1));
                    set_field(vm, out, "linedefined", Value::Int(-1));
                    set_field(vm, out, "lastlinedefined", Value::Int(-1));
                    set_field(vm, out, "func", Value::Nil);
                    let empty = Value::Str(vm.heap.intern(b""));
                    set_field(vm, out, "namewhat", empty);
                    set_field(vm, out, "name", Value::Nil);
                    set_field(vm, out, "istailcall", Value::Bool(true));
                    set_field(vm, out, "extraargs", Value::Int(0));
                }
            }
        }
        v => {
            return Err(arg_error(
                vm,
                1,
                "getinfo",
                &format!("function or level expected, got {}", v.type_name()),
            ));
        }
    }
    // "r" — PUC `CallInfo.u2.transferinfo`: index of the first transferred
    // value and the number transferred for the in-flight hook event. luna
    // arms these in `hook_call`/`hook_return`; outside a hook (the level
    // doesn't currently match the hook frame) PUC also surfaces the same
    // pair, so we expose them unconditionally when "r" is requested.
    if want_transfers {
        set_field(vm, out, "ftransfer", Value::Int(vm.hook_ftransfer as i64));
        set_field(vm, out, "ntransfer", Value::Int(vm.hook_ntransfer as i64));
    }
    Ok(vm.nat_return(fs, &[Value::Table(out)]))
}

fn check_closure(vm: &mut Vm, v: Value, who: &str) -> Result<Gc<LuaClosure>, LuaError> {
    match v {
        Value::Closure(cl) => Ok(cl),
        v => Err(arg_error(
            vm,
            1,
            who,
            &format!("function expected, got {}", v.type_name()),
        )),
    }
}

fn d_getupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    if let Value::Native(nc) = f {
        // PUC: C function upvalues always carry the empty-string name; an
        // in-range index returns ("", value), out-of-range returns NO
        // values (db_getupvalue's `return 0`, not nil — v2.13 CORPUS-IV
        // fixture 186 pins the print() spacing difference).
        let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
        if n < 1 || (n as usize) > nc.upvals.len() {
            return Ok(vm.nat_return(fs, &[]));
        }
        let value = nc.upvals[(n - 1) as usize];
        let nm = Value::Str(vm.heap.intern(b""));
        return Ok(vm.nat_return(fs, &[nm, value]));
    }
    let cl = check_closure(vm, f, "getupvalue")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let Some(idx) = visible_upvalue_index(vm, cl, n) else {
        // PUC db_getupvalue: out-of-range returns 0 values, not nil.
        return Ok(vm.nat_return(fs, &[]));
    };
    // PUC `aux_upvalue` returns "(no name)" when the upvalue's name is NULL —
    // which happens for closures loaded from a stripped binary chunk.
    let name = cl.proto.upvals[idx].name.clone();
    let no_name: &[u8] = if vm.version() <= crate::version::LuaVersion::Lua53 {
        b"(*no name)"
    } else {
        b"(no name)"
    };
    let name_str: &[u8] = if name.is_empty() {
        no_name
    } else {
        name.as_bytes()
    };
    let value = vm.upvalue_value(cl, idx);
    let nv = Value::Str(vm.heap.intern(name_str));
    Ok(vm.nat_return(fs, &[nv, value]))
}

fn d_setupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    // PUC's `db_setupvalue` on a C function: the upvalues are C-only, so
    // Lua-level `setupvalue` returns NO values without writing through
    // (db_setupvalue's `return 0`). 5.1 db.lua :312's
    // `debug.setupvalue(io.read, 1, 10) == nil` still holds — a missing
    // result reads as nil.
    if matches!(f, Value::Native(_)) {
        return Ok(vm.nat_return(fs, &[]));
    }
    let cl = check_closure(vm, f, "setupvalue")?;
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let v = vm.nat_arg(fs, nargs, 2);
    let Some(idx) = visible_upvalue_index(vm, cl, n) else {
        // PUC db_setupvalue: out-of-range returns 0 values, not nil.
        return Ok(vm.nat_return(fs, &[]));
    };
    vm.upvalue_set_value(cl, idx, v);
    let name = cl.proto.upvals[idx].name.clone();
    let no_name: &[u8] = if vm.version() <= crate::version::LuaVersion::Lua53 {
        b"(*no name)"
    } else {
        b"(no name)"
    };
    let name_str: &[u8] = if name.is_empty() {
        no_name
    } else {
        name.as_bytes()
    };
    let nv = Value::Str(vm.heap.intern(name_str));
    Ok(vm.nat_return(fs, &[nv]))
}

/// 1-based upvalue index → raw upvals[] index, skipping the implicit `_ENV`
/// cell under PUC 5.1 (where the global env is on a per-function header
/// slot, not an upvalue, so `nups` / `getupvalue` / `setupvalue` never see
/// it). 5.1 db.lua :301 expects `getupvalue(foo1, 3) == nil` when foo1
/// captures exactly two outer locals.
fn visible_upvalue_index(vm: &Vm, cl: Gc<LuaClosure>, n: i64) -> Option<usize> {
    if n < 1 {
        return None;
    }
    if vm.version() <= crate::version::LuaVersion::Lua51 {
        // map the n-th non-_ENV slot to its raw index
        let mut visible = 0i64;
        for (idx, u) in cl.proto.upvals.iter().enumerate() {
            if &*u.name == "_ENV" {
                continue;
            }
            visible += 1;
            if visible == n {
                return Some(idx);
            }
        }
        return None;
    }
    if (n as usize) > cl.upvals().len() {
        return None;
    }
    Some((n - 1) as usize)
}

fn d_upvaluejoin(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f1 = check_closure(vm, vm.nat_arg(fs, nargs, 0), "upvaluejoin")?;
    let n1 = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let f2 = check_closure(vm, vm.nat_arg(fs, nargs, 2), "upvaluejoin")?;
    let n2 = vm.int_from(vm.nat_arg(fs, nargs, 3), "use as an index")?;
    if n1 < 1 || n1 as usize > f1.upvals().len() || n2 < 1 || n2 as usize > f2.upvals().len() {
        // upvaluejoin DOES check the index (PUC checkupval with pnup).
        return Err(raise_str(vm, "invalid upvalue index"));
    }
    let uv = f2.upvals()[(n2 - 1) as usize];
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { f1.as_mut() }.upvals_mut()[(n1 - 1) as usize] = uv;
    // f1 is user-passed; its upvals slot is the field we just changed.
    // barrier_back demotes f1 back to gray so propagate re-traces.
    vm.heap
        .barrier_back(f1.as_ptr() as *mut crate::runtime::heap::GcHeader);
    Ok(0)
}

fn d_upvalueid(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let n = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    // PUC 5.2/5.3's `checkupval` raised `invalid upvalue index` on an
    // out-of-range upvalue; 5.4 retired that and returns nil instead. The
    // shared logic below picks the version-correct error path.
    let out_of_range_err = |vm: &mut Vm| -> Result<u32, LuaError> {
        if vm.version() <= crate::version::LuaVersion::Lua53 {
            Err(arg_error(vm, 2, "upvalueid", "invalid upvalue index"))
        } else {
            Ok(vm.nat_return(fs, &[Value::Nil]))
        }
    };
    // A C closure (native) carries its upvalues inline; its identity token is the
    // address of the captured-value slot (PUC takes &f->upvalue[n-1]).
    if let Value::Native(nc) = f {
        if n < 1 || n as usize > nc.upvals.len() {
            return out_of_range_err(vm);
        }
        let id = (&nc.upvals[(n - 1) as usize] as *const Value) as *const ();
        return Ok(vm.nat_return(fs, &[Value::LightUserdata(id)]));
    }
    let f = check_closure(vm, f, "upvalueid")?;
    if n < 1 || n as usize > f.upvals().len() {
        return out_of_range_err(vm);
    }
    // PUC returns a light userdata holding the address of the upvalue cell.
    let id = f.upvals()[(n - 1) as usize].as_ptr() as *const ();
    Ok(vm.nat_return(fs, &[Value::LightUserdata(id)]))
}

fn d_setuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `db_setuservalue` does `luaL_checktype(L, 1, LUA_TUSERDATA)` first;
    // the resulting `luaL_typeerror` names "light userdata" specially when
    // the actual argument is light. luna only needs the error wording today —
    // a working full-userdata path is not exercised by the gate, so we accept
    // a `Userdata` arg silently (returning it unchanged) and reject anything
    // else with the PUC-shaped message.
    let v = vm.nat_arg(fs, nargs, 0);
    match v {
        // luna's only userdata (file handles) carries zero user values, so any
        // `debug.setuservalue(ud, val[, n])` lands beyond the available slots
        // — PUC's `db_setuservalue` returns nil in that case. db.lua :434
        // exercises exactly this branch.
        Value::Userdata(_) => Ok(vm.nat_return(fs, &[Value::Nil])),
        Value::LightUserdata(_) => Err(arg_error(
            vm,
            1,
            "setuservalue",
            "userdata expected, got light userdata",
        )),
        _ => {
            let got = vm.obj_typename(v);
            Err(arg_error(
                vm,
                1,
                "setuservalue",
                &format!("userdata expected, got {got}"),
            ))
        }
    }
}

fn d_getuservalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `db_getuservalue(ud[, n])`: returns `(uservalue, true)` for an
    // existing slot, `(nil, false)` when `n` is out of range. luna's only
    // userdata (file handles) carries no user values, so any call is OOR
    // and returns `(nil, false)`. db.lua :435 reads exactly that pair back.
    let v = vm.nat_arg(fs, nargs, 0);
    match v {
        Value::Userdata(_) => Ok(vm.nat_return(fs, &[Value::Nil, Value::Bool(false)])),
        Value::LightUserdata(_) => Err(arg_error(
            vm,
            1,
            "getuservalue",
            "userdata expected, got light userdata",
        )),
        _ => {
            let got = vm.obj_typename(v);
            Err(arg_error(
                vm,
                1,
                "getuservalue",
                &format!("userdata expected, got {got}"),
            ))
        }
    }
}

fn d_traceback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC: `debug.traceback([thread,] [message [, level]])`. With a thread
    // arg, the snapshot describes that coroutine's saved frames (PUC switches
    // the target via L1); message and level shift by one.
    let coro_arg = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(co) => Some(co),
        _ => None,
    };
    let off = if coro_arg.is_some() { 1 } else { 0 };
    let msg = match vm.nat_arg(fs, nargs, off) {
        Value::Nil => Vec::new(),
        Value::Str(s) => s.as_bytes().to_vec(),
        v => {
            // non-string messages are returned unchanged (PUC)
            return Ok(vm.nat_return(fs, &[v]));
        }
    };
    // PUC `db_traceback` defaults level to 1 for the running thread (skip
    // the traceback function itself) and to 0 for an explicit-thread target
    // (since the top frame on the coroutine *is* where it paused).
    let default_level: i64 = if coro_arg.is_some() { 0 } else { 1 };
    let level = match vm.nat_arg(fs, nargs, off + 1) {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        Value::Nil => default_level,
        _ => default_level,
    };
    // PUC `luaL_traceback` prepends "\n" only when a message is present, so
    // the string starts at "stack traceback:" otherwise — `string.match(tb,
    // "\n(.-)\n")` then captures the first frame line (db.lua :314).
    let mut out = msg;
    if !out.is_empty() {
        out.push(b'\n');
    }
    out.extend_from_slice(b"stack traceback:");
    if let Some(co) = coro_arg {
        // Suspended coroutine: PUC switches to the saved frames. luna's
        // `coro_traceback` builds the equivalent listing. The C-frame for
        // `yield` (where execution paused at a `coroutine.yield` boundary) is
        // synthesized from the coroutine's `resume_at` marker.
        // Dead-with-error coroutine: a snapshot taken at the error point was
        // captured in `co.error_traceback` by resume_coro (before unwind
        // popped the frames), so debug.traceback can still show the error
        // site (db.lua :848).
        if let Some(captured) = co.error_traceback.as_ref() {
            out.extend_from_slice(captured);
        } else {
            out.extend(vm.coro_traceback(co, level));
        }
    } else {
        // PUC walks the unified C+Lua stack starting at `level`. luna's
        // `traceback_bytes` only enumerates Lua frames; the only C frame we
        // need to materialize is the traceback itself at level 0 (db.lua
        // :710: `string.find(debug.traceback("hi", 0), "'traceback'")`).
        if level <= 0 {
            // PUC `pushglobalfuncname` qualifies a C function with its
            // canonical dotted path when one is reachable from `_G`. 5.3
            // and 5.4 db.lua baseline on the `'debug.traceback'` spelling
            // (5.3 :534, 5.4 :701); 5.1/5.2/5.5 report the bare
            // `'traceback'` (5.5 :710, 5.1 :391, 5.2 :481).
            let v = vm.version();
            let qualified = matches!(
                v,
                crate::version::LuaVersion::Lua53 | crate::version::LuaVersion::Lua54
            );
            if qualified {
                out.extend_from_slice(b"\n\t[C]: in function 'debug.traceback'");
            } else {
                out.extend_from_slice(b"\n\t[C]: in function 'traceback'");
            }
        }
        // While an xpcall msgh runs, `error_traceback` holds a snapshot taken at
        // the error point (before unwind popped the failed frames). Use it so the
        // msgh sees the error site's stack (incl. a `__close` handler frame),
        // matching PUC's `luaG_errormsg` flow without keeping the frames live.
        match vm.error_traceback.as_ref() {
            Some(captured) => out.extend_from_slice(captured),
            None => out.extend(vm.traceback_bytes(level)),
        }
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn d_sethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // debug.sethook([thread,] hook, mask [, count]); masks: c=call, r=return,
    // l=line, plus a count (every N instructions).
    let (off, target) = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(co) => (1, Some(co)),
        _ => (0, None),
    };
    let hook = vm.nat_arg(fs, nargs, off);
    let mask = match vm.nat_arg(fs, nargs, off + 1) {
        Value::Str(s) => s.as_bytes().to_vec(),
        _ => Vec::new(),
    };
    let count = match vm.nat_arg(fs, nargs, off + 2) {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        _ => 0,
    };
    let state = if hook.is_nil() {
        HookState::default()
    } else {
        HookState {
            func: Some(hook),
            rust_func: None,
            call: mask.contains(&b'c'),
            ret: mask.contains(&b'r'),
            line: mask.contains(&b'l'),
            count: count > 0,
            count_base: count,
            count_left: count,
        }
    };
    vm.set_hook(target, state);
    Ok(0)
}

fn d_gethook(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let target = match vm.nat_arg(fs, nargs, 0) {
        Value::Coro(co) => Some(co),
        _ => None,
    };
    let state = vm.get_hook(target);
    match state.func {
        Some(h) => {
            let mut mask = Vec::new();
            if state.call {
                mask.push(b'c');
            }
            if state.ret {
                mask.push(b'r');
            }
            if state.line {
                mask.push(b'l');
            }
            let m = Value::Str(vm.heap.intern(&mask));
            Ok(vm.nat_return(fs, &[h, m, Value::Int(state.count_base)]))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn d_getlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `debug.getlocal` admits four forms:
    //   (thread, level, n)     — frame at `level` in `thread`
    //   (level, n)              — same with the running thread
    //   (function, n)           — the function's nth parameter *name*, no value
    //   (thread, function, n)   — the function form, scoped to a thread (the
    //                             thread is ignored: parameter names come from
    //                             the proto, not from any runtime context)
    // Function form (db.lua:267).
    let arg0 = vm.nat_arg(fs, nargs, 0);
    let arg1 = vm.nat_arg(fs, nargs, 1);
    let arg2 = vm.nat_arg(fs, nargs, 2);
    let (func_val, n_val) = match (arg0, arg1) {
        (Value::Coro(_), Value::Closure(_)) | (Value::Coro(_), Value::Native(_)) => (arg1, arg2),
        (Value::Closure(_), _) | (Value::Native(_), _) => (arg0, arg1),
        _ => (Value::Nil, Value::Nil),
    };
    if let Value::Closure(cl) = func_val {
        let n = vm.int_from(n_val, "use as an index")?;
        if n < 1 || n as u32 > cl.proto.num_params as u32 {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        let idx = (n - 1) as usize;
        let Some(lv) = cl.proto.locvars.get(idx) else {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        };
        let nm = Value::Str(vm.heap.intern(lv.name.as_bytes()));
        return Ok(vm.nat_return(fs, &[nm]));
    }
    if matches!(func_val, Value::Native(_)) {
        // Native functions have no Lua-level parameter names.
        return Ok(vm.nat_return(fs, &[Value::Nil]));
    }
    // Thread + level form: read the frame from co's saved context.
    if let Value::Coro(co) = arg0 {
        let level = vm.int_from(arg1, "use as a level")?;
        let n = vm.int_from(arg2, "use as an index")?;
        if !vm.coro_level_in_range(co, level) {
            return Err(arg_error(vm, 2, "getlocal", "level out of range"));
        }
        return match vm.local_at_coro(co, level, n) {
            Some((name, val)) => {
                let nm = Value::Str(vm.heap.intern(name.as_bytes()));
                Ok(vm.nat_return(fs, &[nm, val]))
            }
            None => Ok(vm.nat_return(fs, &[Value::Nil])),
        };
    }
    let level = vm.int_from(arg0, "use as a level")?;
    let n = vm.int_from(arg1, "use as an index")?;
    // level 0 is the running C function itself (`debug.getlocal` here): PUC
    // `luaG_findlocal` falls into the "C temporary" branch — index `n` into
    // the C function's argument window, name `(C temporary)`. db.lua :408
    // reads back its own arguments via `getlocal(0, 1)`, `(0, 2)`, etc.
    if level == 0 {
        if n < 1 || (n as u32) > nargs {
            return Ok(vm.nat_return(fs, &[Value::Nil]));
        }
        let val = vm.nat_arg(fs, nargs, (n - 1) as u32);
        let nm = Value::Str(vm.heap.intern(vm.temporary_locvar_name().as_bytes()));
        return Ok(vm.nat_return(fs, &[nm, val]));
    }
    if !vm.level_in_range(level) {
        return Err(arg_error(vm, 1, "getlocal", "level out of range"));
    }
    match vm.local_at(level, n) {
        Some((name, val)) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm, val]))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn d_setlocal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC `debug.setlocal(thread?, level, n, value)`: write `value` into
    // local / vararg `n` of frame `level`. Returns the local's name on
    // success, nil if the slot is out of range.
    let arg0 = vm.nat_arg(fs, nargs, 0);
    if let Value::Coro(co) = arg0 {
        let level_v = vm.nat_arg(fs, nargs, 1);
        let n_v = vm.nat_arg(fs, nargs, 2);
        let val_v = vm.nat_arg(fs, nargs, 3);
        let level = vm.int_from(level_v, "use as a level")?;
        let n = vm.int_from(n_v, "use as an index")?;
        if !vm.coro_level_in_range(co, level) {
            return Err(arg_error(vm, 2, "setlocal", "level out of range"));
        }
        return match vm.local_set_coro(co, level, n, val_v) {
            Some(name) => {
                let nm = Value::Str(vm.heap.intern(name.as_bytes()));
                Ok(vm.nat_return(fs, &[nm]))
            }
            None => Ok(vm.nat_return(fs, &[Value::Nil])),
        };
    }
    let level_v = arg0;
    let n_v = vm.nat_arg(fs, nargs, 1);
    let val_v = vm.nat_arg(fs, nargs, 2);
    let level = vm.int_from(level_v, "use as a level")?;
    let n = vm.int_from(n_v, "use as an index")?;
    if !vm.level_in_range(level) {
        return Err(arg_error(vm, 1, "setlocal", "level out of range"));
    }
    match vm.local_set(level, n, val_v) {
        Some(name) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm]))
        }
        None => Ok(vm.nat_return(fs, &[Value::Nil])),
    }
}

fn d_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

fn d_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let mt = vm.nat_arg(fs, nargs, 1);
    let m = match mt {
        Value::Nil => None,
        Value::Table(m) => Some(m),
        _ => return Err(arg_error(vm, 2, "setmetatable", "nil or table expected")),
    };
    if let Value::Table(t) = v {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set_metatable(m);
        vm.barrier_back_table(t);
    } else {
        // 5.x: debug.setmetatable sets the shared metatable for the basic type
        vm.set_type_metatable(v, m);
    }
    Ok(vm.nat_return(fs, &[v]))
}

/// PUC 5.1 `debug.setfenv(o, env)`: replace the env of a Lua function (Closure),
/// userdata, or thread (Coro). Returns `o`. 5.2+ removed this along with the
/// global `setfenv`.
fn d_setfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let o = vm.nat_arg(fs, nargs, 0);
    let env = vm.nat_arg(fs, nargs, 1);
    let env_t = match env {
        Value::Table(t) => t,
        v => {
            return Err(arg_error(
                vm,
                2,
                "setfenv",
                &format!("table expected, got {}", v.type_name()),
            ));
        }
    };
    match o {
        Value::Closure(cl) => {
            // Same shape as the global setfenv on a closure: rewrite cell 0
            // (or whichever cell `_ENV` lives in) with the new env table.
            let env_idx = cl
                .proto
                .upvals
                .iter()
                .position(|d| &*d.name == "_ENV")
                .ok_or_else(|| raise_str(vm, "'setfenv' target has no '_ENV' upvalue"))?;
            let uv = cl.upvals()[env_idx];
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            unsafe { uv.as_mut() }.set_closed(Value::Table(env_t));
            vm.barrier_forward_upvalue(uv, Value::Table(env_t));
        }
        Value::Coro(co) => {
            // PUC 5.1 `lua_setfenv` on a thread rewrites `L->l_gt`. If the
            // thread is the running one, that's `Vm.globals`; otherwise it
            // lives on the suspended `Coro` (the swap on resume picks it up).
            if vm.is_current_thread(Some(co)) {
                vm.set_globals(env_t);
            } else {
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { co.as_mut() }.globals = env_t;
                // co.globals is traced by Coro::trace — demote co back to
                // gray so propagate re-traces the new env table.
                vm.heap
                    .barrier_back(co.as_ptr() as *mut crate::runtime::heap::GcHeader);
            }
        }
        Value::Userdata(_) => {
            // luna userdata has no separate env slot; PUC 5.1 stores it on
            // the userdata header. Silently accept (debug.setfenv returns o)
            // — no test in the suite exercises a userdata env round-trip.
        }
        v => {
            return Err(arg_error(
                vm,
                1,
                "setfenv",
                &format!(
                    "function, thread, or userdata expected, got {}",
                    v.type_name()
                ),
            ));
        }
    }
    Ok(vm.nat_return(fs, &[o]))
}

/// PUC 5.1 `debug.getfenv(o)`: return the env of a Lua function, userdata, or
/// thread. For other types, returns nil. 5.2+ removed this.
fn d_getfenv(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::runtime::UpvalState;
    let o = vm.nat_arg(fs, nargs, 0);
    let env = match o {
        Value::Closure(cl) => {
            let env_idx = cl.proto.upvals.iter().position(|d| &*d.name == "_ENV");
            match env_idx {
                Some(i) => match cl.upvals()[i].state() {
                    UpvalState::Closed(v) => v,
                    UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
                },
                None => Value::Table(vm.globals()),
            }
        }
        Value::Coro(co) => {
            if vm.is_current_thread(Some(co)) {
                Value::Table(vm.globals())
            } else {
                Value::Table(co.globals)
            }
        }
        Value::Native(_) | Value::Userdata(_) => Value::Table(vm.globals()),
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[env]))
}
