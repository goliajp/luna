//! `string.gsub` and its replacement forms.

use super::*;

pub(crate) fn s_gsub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    let mut slotted = vm.native_buffinit(0);
    // a string or number replacement is a template
    let template = match repl {
        Value::Str(t) => Some(t),
        Value::Int(_) | Value::Float(_) => Some(argcheck::check_string(vm, a, 2)?),
        _ => None,
    };
    let src = s.as_bytes();
    let (anchor, body) = pattern::anchor_split(pattern_bytes(v, p.as_bytes()));
    let mut ms = MatchState::new(src, body, flavor(v));
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut n: i128 = 0;
    let mut last: Option<usize> = None;
    let mut changed = false;
    while n < max_s {
        let m = ms.try_at(pos).map_err(|err| pat_err(vm, err))?;
        // 5.3 rejects an empty match right after the previous match; earlier
        // versions take it and then copy a byte
        let m = match m {
            Some(e) if v >= LuaVersion::Lua53 && last == Some(e) => None,
            m => m,
        };
        if let Some(e) = m {
            n += 1;
            changed |= add_value(vm, &ms, src, pos, e, repl, template, &mut out)?;
            last = Some(e);
        }
        vm.native_buffgrown(&mut slotted, out.len());
        match m {
            Some(e) if v >= LuaVersion::Lua53 || e > pos => pos = e,
            _ if pos < src.len() => {
                out.push(src[pos]);
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
        out.extend_from_slice(&src[pos..]);
        vm.built_str(&out)?
    };
    Ok(vm.nat_return(fs, &[res, Value::Int(n as i64)]))
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
    out: &mut Vec<u8>,
) -> Result<bool, LuaError> {
    if let Some(t) = template {
        add_s(vm, ms, src, s, e, t.as_bytes(), out)?;
        return Ok(true);
    }
    // the value ends up pushed where the key or the function was
    let r = match repl {
        Value::Table(_) => {
            let k = ms.get_capture(0, s, e).map_err(|err| pat_err(vm, err))?;
            let k = cap_value(vm, src, k);
            vm.native_push(1);
            vm.index_value(repl, k)?
        }
        f => {
            let mut args = Vec::new();
            // the function, then its arguments, are pushed for the call
            vm.native_push(1);
            push_captures(vm, ms, src, s, e, true, &mut args)?;
            vm.native_pop(1);
            // an unprotected C call: the replacement cannot yield
            let r = vm
                .call_value(f, &args)?
                .first()
                .copied()
                .unwrap_or(Value::Nil);
            vm.native_push(1);
            r
        }
    };
    let kept = match r {
        Value::Nil | Value::Bool(false) => {
            out.extend_from_slice(&src[s..e]);
            false
        }
        Value::Str(x) => {
            out.extend_from_slice(x.as_bytes());
            true
        }
        n @ (Value::Int(_) | Value::Float(_)) => {
            let b = vm.tostring_basic(n);
            out.extend_from_slice(&b);
            true
        }
        other => {
            let msg = format!("invalid replacement value (a {})", other.type_name());
            return Err(raise_str(vm, &msg));
        }
    };
    vm.native_pop(1);
    Ok(kept)
}

/// PUC `add_s`: expand `%0`-`%9` and `%%` in a template. 5.1 copies any
/// other escaped byte literally; later versions reject it.
pub(crate) fn add_s(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    s: usize,
    e: usize,
    t: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), LuaError> {
    let lenient = vm.version() == LuaVersion::Lua51;
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        i += 1;
        if c != b'%' {
            out.push(c);
            continue;
        }
        // the template's terminating zero follows a final '%'
        let d = t.get(i).copied().unwrap_or(0);
        i += 1;
        match d {
            b'0' => out.extend_from_slice(&src[s..e]),
            b'1'..=b'9' => {
                let c = ms
                    .get_capture((d - b'1') as usize, s, e)
                    .map_err(|err| pat_err(vm, err))?;
                match c {
                    CapValue::Span(a, b) => out.extend_from_slice(&src[a..b]),
                    CapValue::Pos(p) => out.extend_from_slice((p + 1).to_string().as_bytes()),
                }
            }
            b'%' => out.push(b'%'),
            d if lenient => out.push(d),
            _ => return Err(raise_str(vm, "invalid use of '%' in replacement string")),
        }
    }
    Ok(())
}
