//! `string.gsub`'s template replacement (PUC `add_s`), added to the
//! `luaL_Buffer` as PUC adds it: up to 5.3 a character at a time, with a
//! capture pushed as a string first; from 5.4 the text between escapes at
//! once.

use super::*;

/// Expand `%0`-`%9` and `%%` in template `t` for the match `[s, e)`. 5.1
/// copies any other escaped byte literally; later versions reject it.
pub(super) fn add_s(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    (s, e): (usize, usize),
    t: &[u8],
    (out, b): (&mut LVec<u8>, &mut PucBuffer),
) -> Result<(), LuaError> {
    if vm.version() >= LuaVersion::Lua54 {
        return add_s_54(vm, ms, src, (s, e), t, (out, b));
    }
    let lenient = vm.version() == LuaVersion::Lua51;
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        i += 1;
        if c != b'%' {
            vm.buf_push(out, &[c])?;
            b.add_chars(vm, out, 1)?;
            continue;
        }
        // the template's terminating zero follows a final '%'
        let d = t.get(i).copied().unwrap_or(0);
        i += 1;
        match d {
            b'0' => {
                vm.buf_push(out, &src[s..e])?;
                b.add(vm, out, e - s)?;
            }
            b'1'..=b'9' => {
                let c = ms
                    .get_capture((d - b'1') as usize, s, e)
                    .map_err(|err| pat_err(vm, err))?;
                // `push_onecapture`, then (5.3) `luaL_tolstring` of it: the
                // capture is on PUC's stack meanwhile
                let v = cap_value(vm, src, c)?;
                let r = vm.rooted(&[v], |vm| add_capture(vm, v, (out, b)));
                r?;
            }
            b'%' => {
                vm.buf_push(out, b"%")?;
                b.add_chars(vm, out, 1)?;
            }
            d if lenient => {
                vm.buf_push(out, &[d])?;
                b.add_chars(vm, out, 1)?;
            }
            _ => return Err(invalid_escape(vm)),
        }
    }
    Ok(())
}

/// Capture value `v`, rooted by the caller, added as a string.
fn add_capture(
    vm: &mut Vm,
    v: Value,
    (out, b): (&mut LVec<u8>, &mut PucBuffer),
) -> Result<(), LuaError> {
    let x = match v {
        _ if vm.version() == LuaVersion::Lua53 => vm.tolstring(v, 0)?,
        Value::Str(x) => x,
        // `luaL_addvalue`'s `lua_tolstring`
        n => vm.number_string(n)?,
    };
    vm.buf_push(out, x.as_bytes())?;
    b.add_value(vm, out, x)
}

/// 5.4 on: each run of text up to an escape added at once.
fn add_s_54(
    vm: &mut Vm,
    ms: &MatchState,
    src: &[u8],
    (s, e): (usize, usize),
    t: &[u8],
    (out, b): (&mut LVec<u8>, &mut PucBuffer),
) -> Result<(), LuaError> {
    let mut rest = t;
    while let Some(at) = rest.iter().position(|&c| c == b'%') {
        vm.buf_push(out, &rest[..at])?;
        b.add(vm, out, at)?;
        // the template's terminating zero follows a final '%'
        let d = rest.get(at + 1).copied().unwrap_or(0);
        match d {
            b'%' => {
                vm.buf_push(out, b"%")?;
                b.add_chars(vm, out, 1)?;
            }
            b'0' => {
                vm.buf_push(out, &src[s..e])?;
                b.add(vm, out, e - s)?;
            }
            b'1'..=b'9' => {
                let c = ms
                    .get_capture((d - b'1') as usize, s, e)
                    .map_err(|err| pat_err(vm, err))?;
                match c {
                    CapValue::Span(x, y) => {
                        vm.buf_push(out, &src[x..y])?;
                        b.add(vm, out, y - x)?;
                    }
                    // pushed as an integer, which `luaL_addvalue` converts
                    CapValue::Pos(p) => {
                        let x = vm.number_string(Value::Int(p as i64 + 1))?;
                        vm.buf_push(out, x.as_bytes())?;
                        b.add_value(vm, out, x)?;
                    }
                }
            }
            _ => return Err(invalid_escape(vm)),
        }
        rest = rest.get(at + 2..).unwrap_or(&[]);
    }
    vm.buf_push(out, rest)?;
    b.add(vm, out, rest.len())
}

/// `"invalid use of '%c' in replacement string"`
fn invalid_escape(vm: &mut Vm) -> LuaError {
    use crate::runtime::puc_fmt::FmtArg;
    let fmt = b"invalid use of '%c' in replacement string";
    crate::vm::builtins::raise_lib(vm, fmt, &[FmtArg::C(b'%')], 1)
}
