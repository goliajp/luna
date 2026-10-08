//! `luaL_checkoption` and the stack room a C function may ask for.

use super::*;

/// `luaL_checkoption`: the index of argument `i` in `options`, read with
/// `luaL_optstring(def)` when a default is given, else `luaL_checkstring`.
pub(crate) fn check_option(
    vm: &mut Vm,
    a: Args,
    i: u32,
    def: Option<&str>,
    options: &[&str],
) -> Result<usize, LuaError> {
    let name = match def {
        Some(d) => match opt_string(vm, a, i)? {
            Some(s) => s.as_bytes().to_vec(),
            None => d.as_bytes().to_vec(),
        },
        None => check_string(vm, a, i)?.as_bytes().to_vec(),
    };
    // The name is a C string to `strcmp` and `%s`: it ends at the first NUL.
    let name = name
        .split(|&b| b == 0)
        .next()
        .expect("split yields a first piece");
    if let Some(k) = options.iter().position(|o| o.as_bytes() == name) {
        return Ok(k);
    }
    let shown = String::from_utf8_lossy(name);
    // the message is pushed before `luaL_argerror` adds to it
    vm.native_push(1);
    Err(arg_error(vm, i + 1, &format!("invalid option '{shown}'")))
}
