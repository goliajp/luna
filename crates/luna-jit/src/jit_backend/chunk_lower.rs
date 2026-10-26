use super::*;

mod cfg;
mod checks;
mod emit;
mod emit_alloc;
mod emit_basic;
mod emit_calls;
mod emit_entry;
mod emit_for;
mod emit_for_loop;
mod emit_set_list;
mod emit_table_get;
mod emit_table_set;
mod entry;
mod handoff;
mod helpers;
mod kind_flow;
mod kinds;
mod kinds_for;
mod kinds_ops;
mod narrow;
mod nil_flow;
mod scan;
mod scan_data;
mod scan_ops;
mod scan_tables;
mod table_flow;
use cfg::ChunkCfg;
use emit::{EmitFacts, EmitState};
use emit_alloc::emit_hand_if_zero;
use entry::*;
use helpers::*;
use kinds::KindSweep;
use scan::ChunkScan;

/// The chunk being lowered, as every pass reads it.
#[derive(Clone, Copy)]
struct ChunkIn<'a> {
    proto: Gc<Proto>,
    code: &'a [Inst],
    /// each instruction's pc in `proto.code` (see `split_const_operands`)
    old_pc: &'a [u32],
    n: usize,
    num_params: usize,
    max_stack: usize,
    pre53: bool,
    float_only: bool,
}

/// The kinds of the `n` parameters as the dispatcher passes them: a bit
/// per float and per table (a table's raw `Gc<Table>` in its i64 slot);
/// any other, an unread one included, is an integer.
fn arg_masks(reg_kinds: &[RegKind], n: usize) -> (u8, u8) {
    let (mut float, mut table) = (0u8, 0u8);
    for (i, k) in reg_kinds.iter().take(n).enumerate() {
        match k {
            RegKind::Float => float |= 1 << i,
            RegKind::Table => table |= 1 << i,
            _ => {}
        }
    }
    (float, table)
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
    let (code, old_pc) = const_operands::split_const_operands(&proto, first_scratch)?;
    let (code, old_pc) = (&code[..], &old_pc[..]);
    let n = code.len();
    if n == 0 {
        return None;
    }
    // the scratch registers of `split_const_operands`
    let max_stack = (proto.max_stack as usize).max(num_params) + const_operands::SCRATCH_REGS;
    let c = ChunkIn {
        proto,
        code,
        old_pc,
        n,
        num_params,
        max_stack,
        pre53,
        float_only,
    };
    let scan = scan::scan_chunk(c)?;
    let cfg = cfg::build_cfg(c, &scan)?;
    // compiled code returns one value or none, the same on every path: a
    // chunk that returns one on some paths and none on others is left to
    // the interpreter
    if cfg.returns == (true, true) {
        return None;
    }
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
    let (arg_float_mask, arg_table_mask) = arg_masks(&reg_kinds, num_params);
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

    // finalising is the caller's job
    Some((
        entry_id,
        ChunkMeta {
            num_args: num_params as u8,
            returns_one: cfg.returns.1,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        },
    ))
}
