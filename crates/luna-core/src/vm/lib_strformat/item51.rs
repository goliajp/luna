//! One `string.format` conversion under 5.1, 5.2 and 5.3.

use super::*;

/// One conversion under 5.1, 5.2 or 5.3. Returns where scanning resumes;
/// `added` is left as it is ([`Added::Sized`]) unless the item says
/// otherwise. `v` is the running dialect.
#[allow(clippy::too_many_arguments)]
pub(super) fn item51<const PRE: bool>(
    vm: &mut Vm,
    v: LuaVersion,
    a: Args,
    arg: u32,
    fmt: &[u8],
    start: usize,
    out: &mut LVec<u8>,
    added: &mut Added,
) -> Result<usize, LuaError> {
    let p = scanformat(vm, fmt, start)?;
    // every conversion but a long `%s` and `%q` fits in this
    vm.buf_reserve(out, cfmt::ITEM_MAX)?;
    let conv = byte_at(fmt, p);
    let body = &fmt[start..p.min(fmt.len())];
    let sp = Spec::parse(body);
    match conv {
        b'c' => {
            let c = match v {
                LuaVersion::Lua51 => crate::vm::c_abi::to_int(argcheck::check_number(vm, a, arg)?),
                LuaVersion::Lua52 => argcheck::check_int(vm, a, arg)?,
                _ => argcheck::check_integer(vm, a, arg)? as i32,
            };
            let mark = out.len();
            cfmt::char(out, &sp, c as u8);
            // 5.1 copies the item with strlen, so a zero byte ends it
            if v == LuaVersion::Lua51
                && let Some(z) = out[mark..].iter().position(|&b| b == 0)
            {
                out.truncate(mark + z);
            }
        }
        b'd' | b'i' => {
            let n = match v {
                // `(long)`
                LuaVersion::Lua51 => {
                    crate::vm::c_abi::to_long(argcheck::check_number(vm, a, arg)?) as i64
                }
                // PUC casts and checks that the cast lost less than one; a
                // value outside the integer range must fail, which is what
                // the check does where the cast yields the "indefinite"
                // value (the reference test suite asserts it for 2^63)
                LuaVersion::Lua52 => {
                    let x = argcheck::check_number(vm, a, arg)?;
                    if !(-TWO63..TWO63).contains(&x) {
                        return Err(arg_error(vm, arg + 1, "not a number in proper range"));
                    }
                    x as i64
                }
                _ => argcheck::check_integer(vm, a, arg)?,
            };
            cfmt::signed(out, &sp, n);
        }
        b'o' | b'u' | b'x' | b'X' => {
            let n = match v {
                // `(unsigned long)`
                // `unsigned long` is 32 bits on Windows: the cast is one there
                #[allow(clippy::unnecessary_cast)]
                LuaVersion::Lua51 => {
                    crate::vm::c_abi::to_ulong(argcheck::check_number(vm, a, arg)?) as u64
                }
                LuaVersion::Lua52 => {
                    let x = argcheck::check_number(vm, a, arg)?;
                    if !(x > -1.0 && x < 2.0 * TWO63) {
                        return Err(arg_error(
                            vm,
                            arg + 1,
                            "not a non-negative number in proper range",
                        ));
                    }
                    x as u64
                }
                _ => argcheck::check_integer(vm, a, arg)? as u64,
            };
            cfmt::unsigned(out, &sp, conv, n);
        }
        b'a' | b'A' if v >= LuaVersion::Lua52 => {
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'e' | b'E' | b'f' | b'g' | b'G' => {
            let x = argcheck::check_number(vm, a, arg)?;
            cfmt::float(out, &sp, conv, x);
        }
        b'q' => match v {
            LuaVersion::Lua51 | LuaVersion::Lua52 => {
                let s = argcheck::check_string(vm, a, arg)?;
                addquoted(vm, s.as_bytes(), out)?;
                *added = Added::Chars;
            }
            _ => *added = addliteral::<PRE>(vm, a, arg, out)?,
        },
        b's' => {
            if v >= LuaVersion::Lua53
                && body.is_empty()
                && let Some(s) = plain_str::<PRE>(vm, a, arg)?
            {
                vm.buf_push(out, s.as_bytes())?;
                *added = Added::Value(s);
                return Ok(p + 1);
            }
            let gs = if v == LuaVersion::Lua51 {
                Some(argcheck::check_string(vm, a, arg)?)
            } else {
                // a `__tostring` runs above the buffer's slot
                let buf = vm.buffer_slot(out.len());
                if buf > 0 {
                    vm.native_hold_top(&[Vm::buffer_box()])?;
                }
                if v == LuaVersion::Lua52 {
                    tolstring_52::<PRE>(vm, a.get(vm, arg), buf)?
                } else {
                    let v = a.get(vm, arg);
                    Some(vm.tolstring::<PRE>(v, buf)?)
                }
            };
            // the argument itself stays where it is, rooted; a string made
            // from it is copied before the buffer may grow
            let held;
            let s: &[u8] = match &gs {
                None => b"(null)",
                Some(g) if matches!(a.get(vm, arg), Value::Str(x) if x.as_ptr() == g.as_ptr()) => {
                    g.as_bytes()
                }
                Some(g) => {
                    held = g.as_bytes().to_vec();
                    &held
                }
            };
            let has_prec = body.contains(&b'.');
            if v >= LuaVersion::Lua53
                && body.is_empty()
                && let Some(gs) = gs
            {
                vm.buf_push(out, s)?;
                *added = Added::Value(gs);
            } else {
                if v >= LuaVersion::Lua53 && s.contains(&0) {
                    // over the `luaL_tolstring` result
                    vm.native_push(1);
                    return Err(arg_error(vm, arg + 1, "string contains zeros"));
                }
                if !has_prec
                    && s.len() >= 100
                    && let Some(gs) = gs
                {
                    vm.buf_push(out, s)?;
                    *added = Added::Value(gs);
                } else {
                    cfmt::cstr(out, &sp, s);
                }
            }
        }
        _ => {
            // 5.1's "%c" of a NUL is an empty piece (`pushstr` takes its
            // length with `strlen`)
            let fmt = b"invalid option '%%%c' to 'format'";
            return Err(if v == LuaVersion::Lua51 && conv == 0 {
                let fmt = b"invalid option '%%%s' to 'format'";
                crate::vm::builtins::raise_lib(
                    vm,
                    fmt,
                    &[crate::runtime::puc::fmt::FmtArg::S(b"")],
                    2,
                )
            } else {
                crate::vm::builtins::raise_lib(
                    vm,
                    fmt,
                    &[crate::runtime::puc::fmt::FmtArg::C(conv)],
                    2,
                )
            });
        }
    }
    Ok(p + 1)
}

/// C's `(int)` conversion of a double on the reference platform: a
/// saturating one, NaN to zero.
/// 5.2's `luaL_tolstring` does not check what `__tostring` returns: a
/// value that is not a string or a number formats as C's `%s` of a null
/// pointer.
/// The string it leaves pushed; none for such a value.
fn tolstring_52<const PRE: bool>(
    vm: &mut Vm,
    v: Value,
    extra: u32,
) -> Result<Option<crate::runtime::Gc<crate::runtime::LuaStr>>, LuaError> {
    let mm = vm.get_mm(v, crate::vm::exec::Mm::ToString);
    if mm.is_nil() {
        return vm.tolstring::<PRE>(v, extra).map(Some);
    }
    // the slots past the top stay live through the collection it checks for
    vm.native_push(extra);
    let r = vm.metafield_check::<PRE>(v, b"__tostring");
    vm.native_pop(extra);
    r?;
    match vm
        .call_value_pushed(mm, &[v], extra)?
        .first()
        .copied()
        .unwrap_or(Value::Nil)
    {
        Value::Str(s) => Ok(Some(s)),
        // `lua_tolstring` converts it
        r @ (Value::Int(_) | Value::Float(_)) => vm.number_string(r).map(Some),
        _ => Ok(None),
    }
}
