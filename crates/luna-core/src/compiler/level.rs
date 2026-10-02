//! The state of one function being compiled, and the vectors kept between
//! functions and loads.

use super::*;

pub(super) struct Level<'a> {
    pub(super) code: Vec<Inst>,
    pub(super) lines: Vec<u32>,
    pub(super) consts: Vec<Value>,
    pub(super) const_map: ConstMap,
    pub(super) locals: Vec<LocalVar<'a>>,
    /// ordered active-variable sequence (locals + global decls) for goto scope
    pub(super) avars: Vec<AVar<'a>>,
    pub(super) blocks: Vec<BlockCx>,
    pub(super) freereg: u32,
    pub(super) max_stack: u32,
    pub(super) upvals: Vec<UpvalDesc>,
    pub(super) protos: Vec<Gc<Proto>>,
    /// completed local-variable debug records (flushed on scope exit)
    pub(super) locvars: Vec<crate::runtime::LocVar>,
    pub(super) num_params: u8,
    pub(super) is_vararg: bool,
    /// Mirrors PUC `(vararg table)` locvar emission: true only for an explicit
    /// anonymous `(...)` parlist (NOT a main chunk's implicit vararg).
    pub(super) has_vararg_table_pseudo: bool,
    /// PUC 5.1 LUAI_COMPAT_VARARG: the hidden `arg` table local was reserved.
    /// The runtime populates it on entry; see Proto::has_compat_vararg_arg.
    pub(super) has_compat_vararg_arg: bool,
    #[allow(dead_code)]
    pub(super) line_defined: u32,
    /// PUC `fs->lasttarget` equivalent: the highest pc that is the destination
    /// of any patched jump (forward jump landing here, backward jump-back to a
    /// previously saved pc, ForLoop / TForLoop back-edge, or a defined label).
    /// `None` is PUC's sentinel `-1` — no target has been recorded yet.
    ///
    /// Read by peephole passes (see `no_jump_lands_here`) that rewrite the
    /// just-emitted instruction at pc `here() - 1` in place of emitting a
    /// Move at `here()`: that is safe only when no jump lands at `here()`,
    /// i.e. `last_target < here()` or `last_target == None`. Consumed by the
    /// Reloc-landing peephole at `assign_name` and the trailing-Move elision
    /// at `assign_stat`.
    ///
    /// Maintained monotonically (only advances upward) by `mark_target(pc)`,
    /// called from every code path that turns some `pc` into a jump landing
    /// point.
    pub(super) last_target: Option<usize>,
    /// 5.1: the first zero this function loaded as a constant. PUC 5.1 keys
    /// its constant table by number value, where `0 == -0`, so every later
    /// zero, of either sign, loads that one.
    pub(super) zero_51: Option<f64>,
}

impl<'a> Level<'a> {
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
            locals: relabel(bufs.locals),
            avars: relabel(bufs.avars),
            blocks: bufs.blocks,
            freereg: num_params as u32,
            max_stack: (num_params as u32).max(2),
            upvals: bufs.upvals,
            protos: bufs.protos,
            locvars: bufs.locvars,
            num_params,
            is_vararg,
            has_vararg_table_pseudo: false,
            has_compat_vararg_arg: false,
            line_defined,
            last_target: None,
            zero_51: None,
        }
    }

    /// The finished function, and this level's vectors emptied for the next
    /// one. The function's arrays are allocated at their exact size.
    pub(super) fn into_proto(
        mut self,
        source: Gc<LuaStr>,
        line_defined: u32,
        last_line_defined: u32,
    ) -> (Proto, LevelBufs) {
        crate::runtime::function_close::mark_closing_returns(&mut self.code, &self.protos);
        let env_upval_idx = self
            .upvals
            .iter()
            .take(u8::MAX as usize)
            .position(|u| &*u.name == "_ENV")
            .map_or(u8::MAX, |i| i as u8);
        let proto = Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: self.code.drain(..).collect(),
            consts: self.consts.drain(..).collect(),
            protos: self.protos.drain(..).collect(),
            upvals: self.upvals.drain(..).collect(),
            num_params: self.num_params,
            is_vararg: self.is_vararg,
            has_vararg_table_pseudo: self.has_vararg_table_pseudo,
            has_compat_vararg_arg: self.has_compat_vararg_arg,
            max_stack: self.max_stack as u8,
            lines: self.lines.drain(..).collect(),
            source,
            line_defined,
            last_line_defined,
            locvars: self.locvars.drain(..).collect(),
            cache: std::cell::Cell::new(None),
            jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
            env_upval_idx,
            trace_hot_count: std::cell::Cell::new(0),
            call_hot_count: std::cell::Cell::new(0),
            trace_discard_count: std::cell::Cell::new(0),
            trace_gave_up: std::cell::Cell::new(false),
            trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
            traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
            has_dispatchable_trace: std::cell::Cell::new(false),
            trace_heads: std::cell::Cell::new([crate::runtime::function::TRACE_HEADS_NONE; 2]),
            trace_call_head_settled: std::cell::Cell::new(false),
        };
        self.const_map.clear();
        self.blocks.clear();
        let bufs = LevelBufs {
            code: self.code,
            lines: self.lines,
            consts: self.consts,
            const_map: self.const_map,
            locals: relabel(self.locals),
            avars: relabel(self.avars),
            blocks: self.blocks,
            upvals: self.upvals,
            protos: self.protos,
            locvars: self.locvars,
        };
        (proto, bufs)
    }
}

/// The vectors of a function being compiled, empty, kept from one function
/// (and one load) to the next so that compiling one allocates only the
/// finished function's arrays.
pub(crate) struct LevelBufs {
    code: Vec<Inst>,
    lines: Vec<u32>,
    consts: Vec<Value>,
    const_map: ConstMap,
    locals: Vec<LocalVar<'static>>,
    avars: Vec<AVar<'static>>,
    blocks: Vec<BlockCx>,
    upvals: Vec<UpvalDesc>,
    protos: Vec<Gc<Proto>>,
    locvars: Vec<crate::runtime::LocVar>,
}

impl Default for LevelBufs {
    fn default() -> LevelBufs {
        // sized for a small function, past most of the regrowth steps
        LevelBufs {
            code: Vec::with_capacity(32),
            lines: Vec::with_capacity(32),
            consts: Vec::with_capacity(8),
            const_map: ConstMap::with_capacity_and_hasher(8, Default::default()),
            locals: Vec::with_capacity(8),
            avars: Vec::with_capacity(8),
            blocks: Vec::with_capacity(4),
            upvals: Vec::new(),
            protos: Vec::new(),
            locvars: Vec::new(),
        }
    }
}

/// The empty vector `v` as a vector of the same type under another
/// lifetime, keeping its allocation (the element types have one layout,
/// so collecting reuses the buffer).
pub(super) fn relabel<T, U>(mut v: Vec<T>) -> Vec<U> {
    v.clear();
    v.into_iter().map(|_| unreachable!()).collect()
}

/// What the compiler keeps between loads: the vectors of finished
/// functions ([`LevelBufs`]), one per nesting level reached so far.
#[derive(Default)]
pub(crate) struct CompileScratch {
    pub(super) levels: Vec<LevelBufs>,
    /// the stack of functions being compiled, empty
    pub(super) open: Vec<Level<'static>>,
    pub(super) sym_strs: Vec<Option<Gc<LuaStr>>>,
}
