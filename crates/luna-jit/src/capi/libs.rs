//! The standard libraries one at a time (`lualib.h`'s `luaopen_*`),
//! opening them all (`luaL_openlibs`, 5.5's `luaL_openselectedlibs`), and
//! 5.1/5.2's module functions. The functions are C (`csrc/aux_libs.c`);
//! a library itself is made by luna-core, through `luna_capi_openlib`.

use super::*;

c_exports! {
    luaopen_base => luna_c_luaopen_base,
    luaopen_package => luna_c_luaopen_package,
    luaopen_coroutine => luna_c_luaopen_coroutine,
    luaopen_table => luna_c_luaopen_table,
    luaopen_io => luna_c_luaopen_io,
    luaopen_os => luna_c_luaopen_os,
    luaopen_string => luna_c_luaopen_string,
    luaopen_bit32 => luna_c_luaopen_bit32,
    luaopen_math => luna_c_luaopen_math,
    luaopen_utf8 => luna_c_luaopen_utf8,
    luaopen_debug => luna_c_luaopen_debug,
    luaL_openlibs => luna_c_luaL_openlibs,
    luaL_openselectedlibs => luna_c_luaL_openselectedlibs,
    luaL_makeseed => luna_c_luaL_makeseed,
    luaL_alloc => luna_c_luaL_alloc,
    luaL_findtable => luna_c_luaL_findtable,
    luaL_pushmodule => luna_c_luaL_pushmodule,
    luaL_openlib => luna_c_luaL_openlib,
    luaL_register => luna_c_luaL_register,
}

/// Open the standard library `name` (its global name, `_G` for the base
/// library) as the dialect's `luaopen_*` does, pushing what that function
/// returns; returns how many values. -1 is 5.1's name conflict: the name
/// of the module whose global is not a table is pushed instead, for the C
/// side to raise the error.
///
/// # Safety
/// `L` is a live thread of an open state, and no other API call on it is
/// running other than a C function it is calling into; `name` is a
/// NUL-terminated name of a standard library.
// SAFETY: no other item in the link is named `luna_capi_openlib`; the C
// side is its only caller
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_capi_openlib(L: *mut LuaState, name: *const c_char) -> c_int {
    // SAFETY: the caller's contract (# Safety)
    let mut api = unsafe { Api::new(L) };
    // SAFETY: `name` is NUL-terminated (# Safety)
    let name = unsafe { c_bytes(name) }.unwrap_or_default();
    let name = std::str::from_utf8(name).expect("a library name is ASCII");
    match api.vm.host_open_lib(name) {
        Ok(vals) => {
            api.push_all(&vals);
            vals.len() as c_int
        }
        Err(module) => {
            let s = api.str(module.as_bytes());
            api.push(s);
            -1
        }
    }
}
