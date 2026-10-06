//! PUC's per-function limits on registers, upvalues and locals.

use crate::version::LuaVersion;

/// PUC `luaK_checkstack`'s register cap, as the most registers a function
/// may use: 5.1/5.2 fail at `newstack >= 250` (`MAXSTACK`/`MAXREGS` 250),
/// 5.3/5.4 at `newstack >= 255`, 5.5 at `newstack > 255`.
pub(super) fn max_regs(version: LuaVersion) -> u32 {
    match version {
        LuaVersion::Lua51 | LuaVersion::Lua52 => 249,
        LuaVersion::Lua53 | LuaVersion::Lua54 => 254,
        _ => 255,
    }
}
/// PUC `LUAI_MAXUPVAL`: the per-function upvalue cap. 5.1 set this to 60;
/// 5.2+ raised it to 255 because the bytecode encoding gained the room.
/// Errors raised at this boundary use the standard "too many upvalues
/// (limit is …) in function at line …" format (errors.lua 5.4 :765 /
/// :775; 5.1 :238 walks 70 inner closures and expects to wall at line 3).
pub(super) fn max_upvals(version: LuaVersion) -> u32 {
    if version <= LuaVersion::Lua51 {
        60
    } else {
        255
    }
}
/// PUC `MAXVARS`: the per-function active-locals cap.
pub(super) const MAX_LOCALS: u32 = 200;
