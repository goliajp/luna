//! The metamethod event an instruction can call.

use crate::version::LuaVersion;
use crate::vm::isa::Op;

/// The metamethod event an instruction can call, per PUC
/// `funcnamefromcode` of each version (5.2's `getfuncname`). 5.1 names no
/// metamethod.
pub(crate) fn instr_event(v: LuaVersion, op: Op) -> Option<&'static str> {
    Some(match op {
        Op::SelfOp
        | Op::GetTabUp
        | Op::GetTabUpR
        | Op::GetTable
        | Op::GetTableK
        | Op::GetI
        | Op::GetField => "index",
        Op::SetTabUp
        | Op::SetTabUpR
        | Op::SetTabUpK
        | Op::SetTable
        | Op::SetTableK
        | Op::SetI
        | Op::SetField => "newindex",
        Op::Eq => "eq",
        Op::Add => "add",
        Op::Sub => "sub",
        Op::Mul => "mul",
        Op::Div => "div",
        Op::Mod => "mod",
        Op::Pow => "pow",
        Op::Unm => "unm",
        Op::Len => "len",
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Concat => "concat",
        Op::IDiv if v >= LuaVersion::Lua53 => "idiv",
        Op::BAnd if v >= LuaVersion::Lua53 => "band",
        Op::BOr if v >= LuaVersion::Lua53 => "bor",
        Op::BXor if v >= LuaVersion::Lua53 => "bxor",
        Op::Shl if v >= LuaVersion::Lua53 => "shl",
        Op::Shr if v >= LuaVersion::Lua53 => "shr",
        Op::BNot if v >= LuaVersion::Lua53 => "bnot",
        // luna keeps `Return0`/`Return1` (with `k`) where PUC's
        // `luaK_finish` makes them `OP_RETURN` to close what is open
        Op::Close | Op::Return | Op::Return0 | Op::Return1 if v >= LuaVersion::Lua54 => "close",
        _ => return None,
    })
}
