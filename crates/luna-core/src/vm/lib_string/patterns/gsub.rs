//! `string.gsub` and its replacement forms.

use super::*;
use crate::runtime::mem::LVec;
use crate::vm::puc_buffer::PucBuffer;

mod template;
use template::add_s;

pub(crate) fn s_gsub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // a frame of `sort_scratch`, a GC root, for the replacement in hand
    let mut frame = LVec::new(vm.heap.mem());
    frame.push(Value::Nil)?;
    vm.sort_scratch.push(frame)?;
    let r = gsub(vm, fs, nargs);
    vm.sort_scratch.pop();
    r
}

fn gsub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let p = argcheck::check_string(vm, a, 1)?;
    let repl = a.get(vm, 2);
    let srcl = s.len() as i64;
    let max_s: i128 = match v {
        LuaVersion::Lua51 => i128::from(argcheck::opt_int(vm, a, 3, (srcl + 1) as i32)?),
        // 5.2 keeps the count in a size_t: a negative one is huge
        LuaVersion::Lua52 => i128::from(argcheck::opt_integer(vm, a, 3, srcl + 1)? as u64),
        _ => i128::from(argcheck::opt_integer(vm, a, 3, srcl + 1)?),
    };
    let repl_ok = matches!(
        repl,
        Value::Str(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Table(_)
            | Value::Closure(_)
            | Value::Native(_)
    );
    if !repl_ok {
        return Err(if v >= LuaVersion::Lua54 {
            argcheck::type_error(vm, a, 2, "string/function/table")
        } else {
            arg_error(vm, 3, "string/function/table expected")
        });
    }
    // a string or number replacement is a template
    let template = match repl {
        Value::Str(t) => Some(t),
        Value::Int(_) | Value::Float(_) => Some(argcheck::check_string(vm, a, 2)?),
        _ => None,
    };
    let src = s.as_bytes();
    let (anchor, body) = pattern::anchor_split(pattern_bytes(v, p.as_bytes()));
    let mut ms = MatchState::new(src, body, flavor(v));
    let mut out = vm.new_buf();
    let mut b = PucBuffer::new(vm);
    let mut pos = 0usize;
    let mut n: i128 = 0;
    let mut last: Option<usize> = None;
    let mut changed = false;
    while n < max_s {
        let m = match ms.try_at(pos) {
            Ok(m) => m,
            Err(err) => {
                let buf = vm.buffer_slot(out.len());
                vm.native_push(buf);
                return Err(pat_err(vm, err));
            }
        };
        // 5.3 rejects an empty match right after the previous match; earlier
        // versions take it and then copy a byte
        let m = match m {
            Some(e) if v >= LuaVersion::Lua53 && last == Some(e) => None,
            m => m,
        };
        if let Some(e) = m {
            n += 1;
            // the buffer's slot counts only for a callback or an error
            let buf = vm.buffer_slot(out.len());
            let to = (&mut out, &mut b);
            changed |= add_value(vm, &ms, src, pos, e, repl, template, buf, to)?;
            last = Some(e);
        }
        match m {
            Some(e) if v >= LuaVersion::Lua53 || e > pos => pos = e,
            _ if pos < src.len() => {
                vm.buf_push(&mut out, &[src[pos]])?;
                b.add_chars(vm, &out, 1)?;
                pos += 1;
            }
            _ => break,
        }
        if anchor {
            break;
        }
    }
    // 5.4 hands back the subject itself when nothing was replaced
    let res = if v >= LuaVersion::Lua54 && !changed {
        Value::Str(s)
    } else {
        vm.buf_push(&mut out, &src[pos..])?;
        b.add(vm, &out, src.len() - pos)?;
        if out.len() > crate::runtime::string::MAX_LEN {
            return Err(vm.rt_err("string length overflow"));
        }
        let r = b.finish(vm, &out)?;
        vm.recycle_buf(out);
        Value::Str(r)
    };
    vm.nat_ret(fs, &[res, Value::Int(n as i64)])
}

/// PUC `add_value`: append the replacement for the match `[s, e)`; false
/// when a function or table kept the original text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_value(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    s: usize,
    e: usize,
    repl: Value,
    template: Option<Gc<LuaStr>>,
    buf: u32,
    (out, b): (&mut LVec<u8>, &mut PucBuffer),
) -> Result<bool, LuaError> {
    if let Some(t) = template {
        if let Err(err) = add_s(vm, ms, src, (s, e), t.as_bytes(), (out, b)) {
            vm.native_push(buf);
            return Err(err);
        }
        return Ok(true);
    }
    // the value ends up pushed where the key or the function was, above
    // the buffer's slot
    let r = match repl {
        Value::Table(_) => {
            let k = match ms.get_capture(0, s, e) {
                Ok(k) => k,
                Err(err) => {
                    vm.native_push(buf);
                    return Err(pat_err(vm, err));
                }
            };
            let k = cap_value(vm, src, k)?;
            // the key is pushed for `lua_gettable`
            vm.index_value_pushed(repl, k, buf + 1)?
        }
        f => {
            let mut args = Vec::new();
            // the function, then its arguments, are pushed for the call;
            // a capture error counts the function
            if let Err(err) = push_captures(vm, ms, src, (s, e), true, buf + 1, &mut args) {
                vm.native_push(buf + 1);
                return Err(err);
            }
            // an unprotected C call: the replacement cannot yield
            vm.call_value_pushed(f, &args, buf)?
                .first()
                .copied()
                .unwrap_or(Value::Nil)
        }
    };
    // the value may be held by nothing else now; the scratch frame keeps it
    // rooted while it is copied (a 5.2 buffer collects when it cannot grow)
    vm.sort_scratch.last_mut().expect("gsub frame")[0] = r;
    let kept = match r {
        // up to 5.3 the original text is pushed as a string first
        Value::Nil | Value::Bool(false) if vm.version() <= LuaVersion::Lua53 => {
            let k = vm.api_push_string(&src[s..e])?;
            let Value::Str(k) = k else {
                unreachable!("a pushed string")
            };
            vm.buf_push(out, k.as_bytes())?;
            b.add_value(vm, out, k)?;
            false
        }
        Value::Nil | Value::Bool(false) => {
            vm.buf_push(out, &src[s..e])?;
            b.add(vm, out, e - s)?;
            false
        }
        Value::Str(x) => {
            vm.buf_push(out, x.as_bytes())?;
            b.add_value(vm, out, x)?;
            true
        }
        // `luaL_addvalue`'s `lua_tolstring` converts it
        n @ (Value::Int(_) | Value::Float(_)) => {
            let x = vm.number_string(n)?;
            vm.buf_push(out, x.as_bytes())?;
            b.add_value(vm, out, x)?;
            true
        }
        other => {
            let tn = other.type_name();
            vm.native_push(buf + 1);
            return Err(crate::vm::builtins::raise_bytes_fmt(
                vm,
                1,
                &[b"invalid replacement value (a ", tn.as_bytes(), b")"].concat(),
            ));
        }
    };
    Ok(kept)
}
