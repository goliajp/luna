//! The state of one function being compiled, and the vectors kept between
//! functions and loads.

use super::*;
use crate::runtime::mem::{LVec, MemRef};

mod scratch;
pub(crate) use scratch::CompileScratch;

pub(super) struct Level<'a> {
    pub(super) code: LVec<Inst>,
    pub(super) lines: LVec<u32>,
    pub(super) consts: LVec<Value>,
    pub(super) const_map: ConstMap,
    /// 5.3: each number's first index in `consts`, which one function may
    /// repeat when another took its key in the chunk's one table
    pub(super) num_first: ConstMap,
    pub(super) locals: LVec<LocalVar<'a>>,
    /// ordered active-variable sequence (locals + global decls) for goto scope
    pub(super) avars: LVec<AVar<'a>>,
    pub(super) blocks: LVec<BlockCx<'a>>,
    /// the labels of the open blocks and the pending gotos (PUC
    /// `dyd->label` / `dyd->gt`, for this function)
    pub(super) labels: LVec<LabelDesc<'a>>,
    pub(super) gotos: LVec<LabelDesc<'a>>,
    pub(super) freereg: u32,
    pub(super) max_stack: u32,
    pub(super) upvals: LVec<UpvalDesc>,
    pub(super) protos: LVec<Gc<Proto>>,
    /// completed local-variable debug records (flushed on scope exit)
    pub(super) locvars: LVec<crate::runtime::LocVar>,
    pub(super) num_params: u8,
    pub(super) is_vararg: bool,
    /// PUC's `(vararg table)` local: an explicit anonymous `(...)` only
    pub(super) has_vararg_table_pseudo: bool,
    /// PUC 5.1 `LUAI_COMPAT_VARARG`'s `arg` local (`Proto::has_compat_vararg_arg`)
    pub(super) has_compat_vararg_arg: bool,
    /// 5.5: the function names its `...` (a local whose `vararg` is not
    /// `Varg::No`)
    pub(super) named_vararg: bool,
    pub(super) line_defined: u32,
    /// PUC `fs->lasttarget`: the highest pc any jump lands on (a patched
    /// jump, a loop's back edge, a label), raised only, by `mark_target`;
    /// `None` is PUC's `-1`, no target yet
    pub(super) last_target: Option<usize>,
    /// 5.1: the first zero loaded as a constant, which every later zero of
    /// either sign loads (PUC keys numbers by value, `0 == -0`)
    pub(super) zero_51: Option<f64>,
    /// PUC `fs->jpc` (before 5.4): jumps to the next instruction emitted
    pub(super) jpc: i32,
    /// a jump [`Compiler::discharge_jpc`] could not reach
    pub(super) jump_err: Option<(SyntaxError, near::TokRef)>,
    /// the values with jump lists (see `Exp::Jumps`)
    pub(super) jexps: LVec<JExp>,
    /// a `global` declaration was made in this function (5.5): until then
    /// no name lookup needs to look for one among `avars`
    pub(super) has_global_decl: bool,
    /// locals about to be declared whose parser entries are logged already
    /// ([`Compiler::prelog_localvar`])
    pub(super) prelogged: u32,
}

/// A value and its true and false jump lists (PUC `expdesc` `t` / `f`).
#[derive(Clone, Copy)]
pub(super) struct JExp {
    pub(super) e: Exp,
    pub(super) t: i32,
    pub(super) f: i32,
}

impl<'a> Level<'a> {
    /// This level, its vectors emptied by [`Self::take_proto`], set up for a
    /// new function in place: a level is a few hundred bytes, and moving it
    /// in and out of the stack for every function cost a copy each way.
    pub(super) fn reset(&mut self, num_params: u8, is_vararg: bool, line_defined: u32) {
        self.freereg = num_params as u32;
        self.max_stack = (num_params as u32).max(2);
        self.num_params = num_params;
        self.is_vararg = is_vararg;
        self.has_vararg_table_pseudo = false;
        self.has_compat_vararg_arg = false;
        self.line_defined = line_defined;
        self.last_target = None;
        self.zero_51 = None;
        self.jpc = NO_JUMP;
        self.jump_err = None;
        self.has_global_decl = false;
    }

    pub(super) fn new(
        num_params: u8,
        is_vararg: bool,
        line_defined: u32,
        bufs: LevelBufs,
    ) -> Level<'a> {
        Level {
            code: bufs.code,
            lines: bufs.lines,
            consts: bufs.consts,
            const_map: bufs.const_map,
            num_first: bufs.num_first,
            locals: bufs.locals.recycle(),
            avars: bufs.avars.recycle(),
            blocks: bufs.blocks.recycle(),
            labels: bufs.labels.recycle(),
            gotos: bufs.gotos.recycle(),
            freereg: num_params as u32,
            max_stack: (num_params as u32).max(2),
            upvals: bufs.upvals,
            protos: bufs.protos,
            locvars: bufs.locvars,
            num_params,
            is_vararg,
            has_vararg_table_pseudo: false,
            named_vararg: false,
            has_compat_vararg_arg: false,
            line_defined,
            last_target: None,
            zero_51: None,
            jpc: NO_JUMP,
            jump_err: None,
            jexps: bufs.jexps,
            has_global_decl: false,
            prelogged: 0,
        }
    }

    /// The innermost active variable named `name` is a `global` declaration.
    pub(super) fn global_declared(&self, name: &str) -> bool {
        self.has_global_decl
            && self
                .avars
                .iter()
                .rev()
                .find(|a| a.name == Some(name))
                .is_some_and(|a| a.global)
    }

    /// The finished function, leaving this level's vectors empty for the
    /// next one. The function's arrays are allocated at their exact size.
    pub(super) fn take_proto(
        &mut self,
        source: Gc<LuaStr>,
        line_defined: u32,
        last_line_defined: u32,
        heap: &Heap,
    ) -> Proto {
        crate::runtime::function_close::mark_closing_returns(&mut self.code, &self.protos);
        let env_upval_idx = self
            .upvals
            .iter()
            .take(u8::MAX as usize)
            .position(|u| &*u.name == "_ENV")
            .map_or(u8::MAX, |i| i as u8);
        let proto = Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: heap.block_of(self.code.drain_all()),
            consts: heap.block_of(self.consts.drain_all()),
            protos: heap.block_of(self.protos.drain_all()),
            upvals: heap.block_of(self.upvals.drain_all()),
            num_params: self.num_params,
            is_vararg: self.is_vararg,
            has_vararg_table_pseudo: self.has_vararg_table_pseudo,
            has_compat_vararg_arg: self.has_compat_vararg_arg,
            max_stack: self.max_stack as u8,
            lines: heap.block_of(self.lines.drain_all()),
            source,
            line_defined,
            last_line_defined,
            locvars: heap.block_of(self.locvars.drain_all()),
            cache: std::cell::Cell::new(None),
            jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
            jit_next: std::cell::Cell::new(None),
            env_upval_idx,
            trace_hot_count: std::cell::Cell::new(0),
            call_hot_count: std::cell::Cell::new(0),
            trace_discard_count: std::cell::Cell::new(0),
            trace_gave_up: std::cell::Cell::new(false),
            puc_arrays: std::cell::Cell::new([0; 7]),
            trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
            inlined_protos: std::cell::RefCell::new(Vec::new()),
            puc_names: std::cell::RefCell::new(Vec::new()),
            traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
            has_dispatchable_trace: std::cell::Cell::new(false),
            trace_heads: std::cell::Cell::new(
                [crate::runtime::function::TRACE_HEADS_NONE;
                    crate::runtime::function::TRACE_HEADS_CAP],
            ),
            trace_call_head_settled: std::cell::Cell::new(false),
        };
        self.clear_scopes();
        proto
    }

    /// The constants' index and the scopes of a function compiled, emptied
    /// for the next.
    fn clear_scopes(&mut self) {
        self.const_map.clear();
        self.num_first.clear();
        self.locals.clear();
        self.avars.clear();
        self.blocks.clear();
        self.labels.clear();
        self.gotos.clear();
        self.jexps.clear();
    }

    /// This level's vectors, emptied, to keep for a later load.
    pub(super) fn into_bufs(self) -> LevelBufs {
        LevelBufs {
            code: self.code,
            lines: self.lines,
            consts: self.consts,
            const_map: self.const_map,
            num_first: self.num_first,
            locals: self.locals.recycle(),
            avars: self.avars.recycle(),
            blocks: self.blocks.recycle(),
            labels: self.labels.recycle(),
            gotos: self.gotos.recycle(),
            upvals: self.upvals,
            protos: self.protos,
            locvars: self.locvars,
            jexps: self.jexps,
        }
    }
}

/// The vectors of a function being compiled, empty, kept from one function
/// (and one load) to the next so that compiling one allocates only the
/// finished function's arrays.
pub(crate) struct LevelBufs {
    code: LVec<Inst>,
    lines: LVec<u32>,
    consts: LVec<Value>,
    const_map: ConstMap,
    num_first: ConstMap,
    locals: LVec<LocalVar<'static>>,
    avars: LVec<AVar<'static>>,
    blocks: LVec<BlockCx<'static>>,
    labels: LVec<LabelDesc<'static>>,
    gotos: LVec<LabelDesc<'static>>,
    upvals: LVec<UpvalDesc>,
    protos: LVec<Gc<Proto>>,
    locvars: LVec<crate::runtime::LocVar>,
    jexps: LVec<JExp>,
}

impl LevelBufs {
    /// The instructions there is room for.
    pub(super) fn code_capacity(&self) -> usize {
        self.code.capacity()
    }

    /// Vectors sized for a small function, past most of the regrowth steps.
    pub(super) fn new(mem: MemRef) -> LevelBufs {
        LevelBufs {
            code: LVec::with_capacity_or_abort(mem, 32),
            lines: LVec::with_capacity_or_abort(mem, 32),
            consts: LVec::with_capacity_or_abort(mem, 8),
            const_map: ConstMap::new(mem),
            num_first: ConstMap::new(mem),
            locals: LVec::with_capacity_or_abort(mem, 8),
            avars: LVec::with_capacity_or_abort(mem, 8),
            blocks: LVec::with_capacity_or_abort(mem, 4),
            labels: LVec::new(mem),
            gotos: LVec::new(mem),
            upvals: LVec::new(mem),
            protos: LVec::new(mem),
            locvars: LVec::new(mem),
            jexps: LVec::new(mem),
        }
    }
}
