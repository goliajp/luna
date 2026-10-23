//! Free functions luna's other crates use.

use crate::frontend::ast::Chunk;
use crate::frontend::ast::ExprId;
use crate::frontend::ast::RhsCallScan;
use crate::jit::ExitTag;
use crate::jit::TagResKind;
use crate::runtime::Builtin;
use crate::runtime::LuaStr;
use crate::runtime::heap::Gc;
use crate::stdio::HostStdout;
use crate::vm::Vm;

#[inline]
pub fn walk_rhs_for_calls(chunk: &Chunk, eid: ExprId) -> RhsCallScan {
    crate::frontend::ast::walk_rhs_for_calls(chunk, eid)
}

#[inline]
pub fn metamethod_safe_for_index_lhs(chunk: &Chunk, obj_eid: ExprId, rhs_eid: ExprId) -> bool {
    crate::frontend::ast::metamethod_safe_for_index_lhs(chunk, obj_eid, rhs_eid)
}

/// Whether a trace compiled for `compiled` entry tags admits registers whose
/// tags are `now`, by the dispatcher's rule.
#[inline]
pub fn entry_tags_admit(compiled: &[u8], now: &[u8]) -> bool {
    crate::jit::entry_tags_admit(compiled, now)
}

/// Walk an `exit_tags` slice and classify it for the
/// dispatcher fast path.
#[inline]
pub fn classify_exit_tags(tags: &[ExitTag]) -> TagResKind {
    crate::jit::classify_exit_tags(tags)
}

/// `(array size, hash size)` a `NewTable` with these operands makes (see
/// `new_table_operands`); `None` past the sizes a table can have, which
/// only crafted bytecode asks for.
#[inline]
pub fn new_table_sizes(b: u32, c: u32, k: bool) -> Option<(usize, usize)> {
    crate::runtime::table::new_table_sizes(b, c, k)
}

/// Make 5.5's `#t` take the random steps of its hash part search from
/// `seed`, as PUC 5.5 does from a state's seed, in every Vm of the process;
/// `None` goes back to drawing them from each table's address. Only for
/// comparing with a PUC 5.5 built with a fixed seed.
#[inline]
pub fn set_len_search_seed(seed: Option<u32>) {
    crate::runtime::table::set_len_search_seed(seed)
}

/// From now on, write standard output through the C library's `stdout`
/// (see `HostStdout`). Process-wide, like C's `stdout`; the first call
/// wins.
#[inline]
pub fn use_host_stdout(h: HostStdout) {
    crate::stdio::use_host_stdout(h)
}

/// The library function `math.<name>` is, for the functions a JIT may
/// inline: it replaces the call with its own code only while the field
/// still holds that function, since a program can assign any value to
/// it. `None` for other names.
#[inline]
pub fn math_inlinable_builtin(name: &[u8]) -> Option<Builtin> {
    crate::vm::lib_math::inlinable_builtin(name)
}

/// `string.sub(s, i, j)` past its argument checks, for compiled code:
/// `None`, with a deopt parked, when the string cannot be made (compiled
/// code may not collect in an emergency; the interpreter runs the call
/// again); `None` without one when 5.1 or 5.2 would collect before
/// `lua_pushlstring` makes it, for the interpreter to make the call. From
/// 5.3 the check after it is the compiled code's.
#[inline]
pub fn str_sub(vm: &mut Vm, s: Gc<LuaStr>, i: i64, j: i64) -> Option<Gc<LuaStr>> {
    crate::vm::lib_string::str_sub(vm, s, i, j)
}

/// The library function `string.<name>` is, if it is one the trace JIT
/// may run as a direct call on arguments of known types.
#[inline]
pub fn string_inlinable_builtin(name: &[u8]) -> Option<crate::runtime::Builtin> {
    crate::vm::lib_string::inlinable_builtin(name)
}
