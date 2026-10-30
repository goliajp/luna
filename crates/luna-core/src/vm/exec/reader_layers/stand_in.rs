//! A prototype standing for one PUC's undumper is still filling while a
//! `load` reader is asked for more: the source and first fields read so
//! far, and no code of its own but a return.

use crate::runtime::Gc;
use crate::runtime::function::{JitProtoState, Proto, TRACE_HEADS_CAP, TRACE_HEADS_NONE};
use crate::runtime::heap::{GcHeader, Heap, ObjTag};
use crate::runtime::mem::Oom;
use crate::vm::isa::{Inst, Op};

/// The prototype of `source` defined from line `lines[0]` to `lines[1]`,
/// of `params` parameters and vararg if `vararg` is set.
pub(super) fn proto(
    heap: &mut Heap,
    source: Gc<crate::runtime::LuaStr>,
    lines: [u64; 2],
    params: u64,
    vararg: u64,
) -> Result<Gc<Proto>, Oom> {
    let proto = Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: heap.try_block_of([Inst::iabc(Op::Return0, 0, 0, 0, false)].into_iter())?,
        consts: heap.try_block_of([].into_iter())?,
        protos: heap.try_block_of([].into_iter())?,
        upvals: heap.try_block_of([].into_iter())?,
        num_params: params as u8,
        is_vararg: vararg != 0,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack: 2,
        lines: heap.try_block_of([].into_iter())?,
        source,
        line_defined: lines[0] as u32,
        last_line_defined: lines[1] as u32,
        locvars: heap.try_block_of([].into_iter())?,
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(JitProtoState::Untried),
        jit_next: std::cell::Cell::new(None),
        env_upval_idx: u8::MAX,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        content: Default::default(),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        puc_arrays: std::cell::Cell::new([0; 7]),
        trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
        inlined_protos: std::cell::RefCell::new(Vec::new()),
        puc_names: std::cell::RefCell::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        has_dispatchable_trace: std::cell::Cell::new(false),
        trace_heads: std::cell::Cell::new([TRACE_HEADS_NONE; TRACE_HEADS_CAP]),
        trace_call_head_settled: std::cell::Cell::new(false),
    };
    heap.try_adopt_proto(proto)
}

/// `p`, made by [`proto`], brought up to the fields read now.
pub(super) fn update(p: Gc<Proto>, lines: [u64; 2], params: u64, vararg: u64) {
    // SAFETY: a prototype of luna's own made by `proto`, which nothing reads
    // while it is changed (no code runs here)
    let p = unsafe { p.as_mut() };
    (p.line_defined, p.last_line_defined) = (lines[0] as u32, lines[1] as u32);
    (p.num_params, p.is_vararg) = (params as u8, vararg != 0);
}

/// `p`'s source, now read.
pub(super) fn update_source(p: Gc<Proto>, source: Gc<crate::runtime::LuaStr>) {
    // SAFETY: as in `update`
    unsafe { p.as_mut() }.source = source;
}
