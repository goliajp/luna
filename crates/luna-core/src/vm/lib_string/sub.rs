//! `string.sub`, in the library and as the direct call compiled code makes.

use super::*;

pub(super) fn s_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let i = argcheck::check_integer(vm, a, 1)?;
    let j = argcheck::opt_integer(vm, a, 2, -1)?;
    // `lua_pushlstring`, checked for as PUC checks
    vm.api_check_before()?;
    let r = Value::Str(try_str_sub(vm, s, i, j)?);
    vm.api_check_after(r)?;
    vm.nat_ret(fs, &[r])
}

/// `string.sub(s, i, j)` past its argument checks, for compiled code:
/// `None`, with a deopt parked, when the string cannot be made (compiled
/// code may not collect in an emergency; the interpreter runs the call
/// again); `None` without one when 5.1 or 5.2 would collect before
/// `lua_pushlstring` makes it, for the interpreter to make the call. From
/// 5.3 the check after it is the compiled code's.
#[doc(hidden)]
pub fn str_sub(vm: &mut Vm, s: Gc<LuaStr>, i: i64, j: i64) -> Option<Gc<LuaStr>> {
    if vm.version() <= LuaVersion::Lua52 && vm.step_due() {
        return None;
    }
    let b = sub_bytes(&s, i, j);
    if let Some(r) = vm.heap.find_single(b) {
        return Some(r);
    }
    let r = vm.heap.intern_or_none(b);
    if r.is_none() {
        vm.jit_park_oom();
    }
    r
}

/// [`str_sub`] in the interpreter, raising a memory error when the string
/// cannot be made.
pub(crate) fn try_str_sub(
    vm: &mut Vm,
    s: Gc<LuaStr>,
    i: i64,
    j: i64,
) -> Result<Gc<LuaStr>, LuaError> {
    let b = sub_bytes(&s, i, j);
    match vm.heap.find_single(b) {
        Some(r) => Ok(r),
        None => vm.alloc_str(b),
    }
}

/// The bytes `string.sub(s, i, j)` takes.
fn sub_bytes(s: &Gc<LuaStr>, i: i64, j: i64) -> &[u8] {
    let l = s.len();
    let start = posrelat(i, l).max(1);
    let end = posrelat(j, l).min(l as i64);
    if start <= end {
        &s.as_bytes()[(start - 1) as usize..end as usize]
    } else {
        b""
    }
}

/// The library function `string.<name>` is, if it is one the trace JIT
/// may run as a direct call on arguments of known types.
#[doc(hidden)]
pub fn inlinable_builtin(name: &[u8]) -> Option<crate::runtime::Builtin> {
    match name {
        b"sub" => Some(crate::runtime::Builtin::StringSub),
        _ => None,
    }
}
