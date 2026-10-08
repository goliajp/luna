//! The entry wrapper a compiled chunk is called through.

use super::*;

pub(super) fn define_entry<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    fn_id: FuncId,
    scan: &ChunkScan,
    ring: Option<RingSpec>,
    num_params: usize,
) -> Option<FuncId> {
    let any_self_call = ring.is_some();
    let ChunkScan {
        self_upval_idx,
        math_folds,
        ..
    } = scan;
    // The body's self-recursive calls go straight to its own code, which
    // is the Lua call only while the upvalue they load holds the running
    // closure, and its math folds replace `math.<fn>(...)` by inline code,
    // which is the Lua call only while the field holds the library
    // function. The compiled code is shared by every closure of the proto
    // (and by protos with the same code), so both are checked on each
    // entry from the interpreter. Recursive calls enter the body directly:
    // nothing the body runs can reassign the upvalue or, with no table
    // stores (checked above), a field.
    let mut math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)> = Vec::new();
    for fold in math_folds {
        if !math_fns.iter().any(|&(_, n)| n.ptr_eq(fold.name_key)) {
            math_fns.push((fold.math_key, fold.name_key));
        }
    }
    let checks = EntryChecks {
        self_upval: self_upval_idx.filter(|_| any_self_call),
        math_fns,
        ring,
    };
    let entry_id = if any_self_call || !checks.math_fns.is_empty() {
        define_checked_entry(module, ctx, fn_id, &checks, num_params)?
    } else {
        fn_id
    };
    Some(entry_id)
}
