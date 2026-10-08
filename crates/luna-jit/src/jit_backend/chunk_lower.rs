use super::*;

mod cfg;
mod checks;
mod emit;
mod emit_basic;
mod emit_calls;
mod emit_entry;
mod emit_for;
mod emit_table_get;
mod emit_table_set;
mod entry;
mod helpers;
mod kind_flow;
mod kinds;
mod kinds_for;
mod kinds_ops;
mod nil_flow;
mod scan;
mod scan_data;
mod scan_ops;
mod scan_tables;
mod table_flow;
use cfg::ChunkCfg;
use emit::{EmitFacts, EmitState};
use entry::*;
use helpers::*;
use kinds::KindSweep;
use scan::ChunkScan;

/// The chunk being lowered, as every pass reads it.
#[derive(Clone, Copy)]
struct ChunkIn<'a> {
    proto: Gc<Proto>,
    code: &'a [Inst],
    n: usize,
    num_params: usize,
    max_stack: usize,
    pre53: bool,
    float_only: bool,
}

/// backend-agnostic body of the int-chunk
/// lowerer. Generic over any `cranelift_module::Module` so the same
/// codegen pipeline drives the runtime JIT (`JITModule`,
/// [`try_compile_int_chunk`]) and the AOT pipeline (`ObjectModule` in
/// `luna-aot`).
///
/// Returns `None` when any opcode in the body falls outside the
/// cumulative whitelist (same gate as [`try_compile_int_chunk`]). On
/// success returns the declared [`FuncId`] for the lowered chunk
/// alongside ABI metadata; the caller drives backend-specific
/// finalization (`JITModule::finalize_definitions` /
/// `ObjectModule::finish`).
// cranelift types in the signature: internal to luna crates, not covered by semver
#[doc(hidden)]
pub fn lower_int_chunk_into<M: Module>(
    module: &mut M,
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
) -> Option<(FuncId, ChunkMeta)> {
    if proto.num_params > MAX_JIT_ARITY {
        return None;
    }
    let num_params = proto.num_params as usize;
    // First pass: verify every op is supported AND scan for basic-block
    // boundaries. A BB starts at PC 0, at every jump target, and at the
    // instruction immediately after a terminator (Jmp, Return, or a
    // paired Lt|Le|Eq+Jmp).
    // the passes below read register operands only
    let first_scratch = (proto.max_stack as usize).max(num_params);
    let code = const_operands::split_const_operands(&proto, first_scratch)?;
    let code = &code[..];
    let n = code.len();
    if n == 0 {
        return None;
    }
    // the scratch registers of `split_const_operands`
    let max_stack = (proto.max_stack as usize).max(num_params) + const_operands::SCRATCH_REGS;
    let c = ChunkIn {
        proto,
        code,
        n,
        num_params,
        max_stack,
        pre53,
        float_only,
    };
    let scan = scan::scan_chunk(c)?;
    let cfg = cfg::build_cfg(c, &scan)?;
    table_flow::check_table_operands(c, &cfg)?;
    let presize_for_newtable = checks::presize_hints(c, &scan);
    checks::check_fold_blocks(&scan)?;
    let any_self_call = checks::check_self_call_base_case(c, &scan)?;
    // self calls keep the stack limit in the pinned register: without one
    // (an AOT object module, another target) they are left to the
    // interpreter
    if any_self_call && !module.isa().flags().enable_pinned_reg() {
        return None;
    }
    let (reg_kinds, ret_kind) = kinds::sweep_kinds(c, &scan, &cfg)?;
    // After convergence: derive per-arg kinds + the ret_is_float flag
    // for the cache slot. An arg that's still Unset (param read by
    // nothing) is treated as Int so the dispatcher's masking is
    // well-defined.
    //
    // Table-typed params go through the dispatcher's
    // `Value::Table` marshalling path (`arg_table_mask`); they
    // pass the raw `Gc<Table>` ptr as the i64 ABI slot.
    let mut arg_float_mask: u8 = 0;
    let mut arg_table_mask: u8 = 0;
    for i in 0..num_params {
        match reg_kinds[i] {
            RegKind::Float => arg_float_mask |= 1 << i,
            RegKind::Table => arg_table_mask |= 1 << i,
            _ => {}
        }
    }
    let ret_is_float = matches!(ret_kind, RegKind::Float);
    let ret_is_table = matches!(ret_kind, RegKind::Table);

    let bb_entry_kinds = kind_flow::bb_entry_kinds(
        c,
        &scan,
        &cfg,
        &reg_kinds,
        ret_kind,
        arg_float_mask,
        arg_table_mask,
    );
    let entry_id = emit::emit_chunk(
        module,
        emit::EmitIn {
            c,
            scan: &scan,
            cfg: &cfg,
            reg_kinds: &reg_kinds,
            ret_kind,
            presize_for_newtable: &presize_for_newtable,
            bb_entry_kinds: &bb_entry_kinds,
            arg_float_mask,
            arg_table_mask,
            any_self_call,
        },
    )?;

    // diag of the lowered chunk's shape lives in the runtime
    // wrapper [`try_compile_int_chunk`]. The generic
    // body only emits the function; finalize is the caller's job.
    let _ = ret_kind; // tracked for diag in the JIT wrapper; backend-agnostic here.

    Some((
        entry_id,
        ChunkMeta {
            num_args: num_params as u8,
            returns_one: scan.sees_return1,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        },
    ))
}
