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
    crate::jit::trace_types::adopt::entry_tags_admit(compiled, now)
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

/// `string.sub(s, i, j)` past its argument checks, for compiled code of
/// 5.3 on, whose check after it is the compiled code's: the string's
/// address, its low bit (clear in any string's) set when the count has
/// reached the point a collection step falls due at; 0, with a deopt
/// parked, when the string cannot be made (compiled code may not collect
/// in an emergency; the interpreter runs the call again).
#[inline]
pub fn str_sub(vm: &mut Vm, s: Gc<LuaStr>, i: i64, j: i64) -> i64 {
    match crate::vm::lib_string::str_sub(vm, s, i, j) {
        Some(r) => r.as_ptr() as i64 | i64::from(vm.heap.due_reached()),
        None => 0,
    }
}

/// [`str_sub`] for compiled code of 5.1 and 5.2: also `None`, without a
/// deopt, when they would collect before `lua_pushlstring` makes the
/// string, for the interpreter to make the call.
#[inline]
pub fn str_sub_checked(vm: &mut Vm, s: Gc<LuaStr>, i: i64, j: i64) -> Option<Gc<LuaStr>> {
    crate::vm::lib_string::str_sub_checked(vm, s, i, j)
}

/// The library function `string.<name>` is, if it is one the trace JIT
/// may run as a direct call on arguments of known types.
#[inline]
pub fn string_inlinable_builtin(name: &[u8]) -> Option<crate::runtime::Builtin> {
    crate::vm::lib_string::inlinable_builtin(name)
}

/// Parks an AOT binary's precompiled `trace` on `proto`, for the deploy
/// side's resolver: the dispatcher then enters it at its `head_pc` as it
/// would a trace the JIT compiled. Nothing is checked or deduplicated; a
/// second trace at the same `head_pc` is entered only if the first is not.
///
/// # Safety
/// `trace` meets [`crate::jit::CompiledTrace::from_parts`]'s conditions,
/// it was compiled for `proto`'s code at its `head_pc` (its entry tags,
/// exit tags and window are that code's registers), and `proto` is a live
/// prototype.
pub unsafe fn install_aot_trace(
    vm: &mut Vm,
    proto: Gc<crate::runtime::function::Proto>,
    trace: crate::jit::CompiledTrace,
) {
    vm.install_aot_trace(proto, trace);
}

/// Takes the source `trace` would be compiled from at the optimizing tier,
/// which then stays at its tier until a source is put back.
pub fn take_tier_source(
    trace: &crate::jit::CompiledTrace,
) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    trace.tier_up.as_ref()?.source.borrow_mut().take()
}

/// Gives `trace`, installed, the source it is compiled from at the
/// optimizing tier. A trace with no tier to move to keeps none.
///
/// # Safety
/// `source` was made for `trace` by the backend that compiled it, in the
/// JIT storage of the Vm `trace` is installed on (the code the optimizing
/// tier makes from it is entered with `trace`'s tags and window).
pub unsafe fn put_tier_source(
    trace: &crate::jit::CompiledTrace,
    source: Box<dyn std::any::Any + Send + Sync>,
) {
    if let Some(t) = &trace.tier_up {
        *t.source.borrow_mut() = Some(source);
    }
}

/// The cell holding the optimizing tier's loop of the trace `t` moves on
/// (null until compiled), whose address the baseline code reads at its
/// back edge.
pub fn tier_up_loop_cell(t: &crate::jit::trace_types::tier_up::TierUp) -> &crate::jit::send_compat::TCellPtr {
    &t.looped
}

/// Gives the trace `t` moves on the optimizing tier's loop its code's
/// back edge calls (see [`tier_up_loop_cell`]), and asks for no further
/// move: only the backend that compiled the trace does, from its tier-up
/// or when it installs the trace.
///
/// # Safety
/// `looped` is the optimizing tier's loop compiled for this trace's
/// `loop_entry` (the code `t` belongs to): against its tags and window,
/// sharing its exits and the cells their addresses name (side-trace cells,
/// frame chains, the iteration count, this loop cell). Entered with
/// `reg_state` holding the registers the loop head reads, as that code's
/// back edge calls it, it returns as that code does.
pub unsafe fn set_tier_up_loop(
    t: &crate::jit::trace_types::tier_up::TierUp,
    looped: crate::jit::trace::TraceFn,
) {
    t.looped.set(looped as *const () as *const u8);
    t.tried.set(true);
}
