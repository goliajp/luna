//! PUC's default paths on Windows, where `!` stands for the directory of
//! the running executable, and `setprogdir`, which replaces it in the path
//! `setpath` just pushed.

use super::*;
use crate::vm::opening::Opener;

/// PUC's `LUA_PATH_DEFAULT` (`field` "path") or `LUA_CPATH_DEFAULT` for
/// Windows, `!` still in it.
pub(super) fn windows_default(v: LuaVersion, field: &[u8]) -> Vec<u8> {
    let vdir = match v {
        LuaVersion::Lua53 => "5.3",
        LuaVersion::Lua54 => "5.4",
        _ => "5.5",
    };
    let s = match (v, field) {
        (LuaVersion::Lua51, b"path") => {
            ".\\?.lua;!\\lua\\?.lua;!\\lua\\?\\init.lua;!\\?.lua;!\\?\\init.lua".to_string()
        }
        (LuaVersion::Lua51, _) => ".\\?.dll;!\\?.dll;!\\loadall.dll".to_string(),
        (LuaVersion::Lua52, b"path") => {
            "!\\lua\\?.lua;!\\lua\\?\\init.lua;!\\?.lua;!\\?\\init.lua;.\\?.lua".to_string()
        }
        (LuaVersion::Lua52, _) => "!\\?.dll;!\\loadall.dll;.\\?.dll".to_string(),
        (_, b"path") => format!(
            "!\\lua\\?.lua;!\\lua\\?\\init.lua;!\\?.lua;!\\?\\init.lua;\
             !\\..\\share\\lua\\{vdir}\\?.lua;!\\..\\share\\lua\\{vdir}\\?\\init.lua;\
             .\\?.lua;.\\?\\init.lua"
        ),
        _ => format!("!\\?.dll;!\\..\\lib\\lua\\{vdir}\\?.dll;!\\loadall.dll;.\\?.dll"),
    };
    s.into_bytes()
}

/// The directory of the running executable, as `setprogdir` cuts
/// `GetModuleFileNameA`'s name at its last `\`.
pub(super) fn exe_dir() -> Option<Vec<u8>> {
    let exe = std::env::current_exe().ok()?;
    Some(path::os_bytes(exe.parent()?.as_os_str()))
}

/// `path` with every `!` the executable's directory.
pub(super) fn in_exe_dir(path: &[u8]) -> Vec<u8> {
    match exe_dir() {
        Some(dir) => path::replace(path, b"!", &dir),
        None => path.to_vec(),
    }
}

/// `setprogdir` on the path on top: `luaL_gsub` makes the copy with the
/// executable's directory, and the original is removed.
pub(super) fn setprogdir(vm: &mut Vm, op: &mut Opener) -> Result<(), LuaError> {
    let made = match op.get(vm, -1) {
        Value::Str(s) => in_exe_dir(s.as_bytes()),
        _ => unreachable!("setpath leaves a string"),
    };
    op.pushstring(vm, &made)?;
    op.remove(vm, -2);
    Ok(())
}
