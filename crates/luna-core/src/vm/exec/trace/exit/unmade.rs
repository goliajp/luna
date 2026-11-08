//! What a trace's exit could not make in compiled code (the frames of the
//! functions it inlined, its sunk tables), made by the dispatcher once the
//! trace has returned, where the interpreter's allocation rules apply.

use super::*;
use crate::runtime::mem::{LVec, MemRef, Oom};
use crate::runtime::table::TableError;

/// The most words of the room an exit keeps unmade sunk tables in that one
/// trace's sunk tables take: a trace sinks no more than fits.
pub const SUNK_WORDS_MAX: usize = 256;

/// The words one sunk table takes in that room: its `NewTable`
/// operands, the ghost level of its record, its array and hash sizes, then
/// its array values and its hash keys and values.
pub const fn sunk_words(cap: usize, n_hash: usize) -> usize {
    4 + cap + 2 * n_hash
}

/// What the dispatcher made outside compiled code for exits that could
/// not: diagnostic counts.
#[derive(Clone, Copy, Default, Debug)]
pub struct UnmadeCounts {
    /// Inline exits whose frames compiled code could not all push.
    pub frames: u64,
    /// Sunk tables.
    pub sunk: u64,
    /// Accumulated strings.
    pub accum: u64,
}

/// What the exit a trace took left unmade. The Vm keeps it last, away from
/// the fields the interpreter reads on every op.
pub(crate) struct ExitUnmade {
    /// Anything is: the accumulated string, frames or sunk tables. Written
    /// through [`Vm::set_unmade_any`], which keeps the Vm's run words.
    any: bool,
    /// The accumulated string the exit could not make.
    pub(crate) accum: Option<crate::vm::exec::AccumUnmade>,
    /// What the dispatcher made so far.
    pub(crate) made: UnmadeCounts,
    /// The inlined frames the exit pushed before the frame stack or the
    /// stack could not grow: the rest are the dispatcher's to push.
    pub(crate) frames: Option<u32>,
    /// The values the collector marks while the dispatcher makes what the
    /// exit could not: the sunk tables' contents, then, while frames are
    /// pushed, the trace's registers. Reserved when a trace is installed,
    /// so the exit and the dispatcher never grow it.
    pub(crate) roots: LVec<Value>,
}

impl ExitUnmade {
    /// See `any`.
    #[inline(always)]
    pub(crate) fn any(&self) -> bool {
        self.any
    }

    pub(crate) fn new(mem: MemRef) -> ExitUnmade {
        ExitUnmade {
            any: false,
            accum: None,
            made: UnmadeCounts::default(),
            frames: None,
            roots: LVec::new(mem),
        }
    }
}

/// `v` onto `roots`, which has room for it (see [`Vm::trace_room`]).
pub(super) fn push_root(roots: &mut LVec<Value>, v: Value) {
    roots.push(v).expect("room reserved with the trace");
}

impl Vm {
    /// Sets whether the exit left something unmade.
    #[inline(always)]
    pub(crate) fn set_unmade_any(&mut self, on: bool) {
        self.unmade.any = on;
        (self.jit.words).block(crate::vm::jit_state::BLOCKED_UNMADE, on);
    }

    /// Before a trace of `window` registers is installed: room for what its
    /// exits may leave unmade, and the dispatcher's buffers for a call of
    /// it. A side trace's registers start inside its parent's window, so
    /// twice the widest window covers theirs. A trace there is no room for
    /// is not installed: it would run out where nothing can fail.
    pub(in crate::vm::exec) fn trace_room(&mut self, window: usize) -> Result<(), Oom> {
        let roots = &mut self.unmade.roots;
        debug_assert!(roots.is_empty());
        roots.reserve(SUNK_WORDS_MAX + 2 * window)?;
        let jit = &mut self.jit;
        let have = jit.link_trail.capacity() - jit.link_trail.len();
        jit.link_trail
            .reserve(crate::jit::trace_types::side_cell::LINK_DEPTH.saturating_sub(have))?;
        super::super::dispatch::words::reserve(
            &mut jit.reg_state_buf,
            &mut jit.entry_tags_buf,
            window,
        )
    }

    /// A sunk table the exit could not make (`ops` its `NewTable`'s packed
    /// operands, sunk site number + 1 from bit 32), its contents kept for
    /// the dispatcher: the odd word the exit writes where the table goes.
    /// Allocates nothing: the room was reserved with the trace.
    pub(crate) fn jit_sunk_unmade(
        &mut self,
        ops: i64,
        arr: impl ExactSizeIterator<Item = Value>,
        hash: impl ExactSizeIterator<Item = (Value, Value)>,
    ) -> i64 {
        let level = self.jit.ghost_depth.max(1) - 1;
        let roots = &mut self.unmade.roots;
        let at = roots.len();
        debug_assert!(at + sunk_words(arr.len(), hash.len()) <= roots.capacity());
        push_root(roots, Value::Int(ops));
        push_root(roots, Value::Int(level as i64));
        push_root(roots, Value::Int(arr.len() as i64));
        push_root(roots, Value::Int(hash.len() as i64));
        for v in arr {
            push_root(roots, v);
        }
        for (k, v) in hash {
            push_root(roots, k);
            push_root(roots, v);
        }
        self.set_unmade_any(true);
        ((at as i64) << 1) | 1
    }

    /// After the write-back of an exit that left something unmade, from
    /// frame `base_us` (the exit's window from its register `off`): the
    /// sunk tables and the accumulated string made now, or dropped when
    /// `failed` holds the error making a frame raised. Then the pc the
    /// frame on top raises that error at.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_exit_unmade_after(
        &mut self,
        base_us: usize,
        off: usize,
        failed: Option<(LuaError, u32)>,
    ) -> Option<u32> {
        self.set_unmade_any(false);
        let drop = failed.is_some();
        // nothing collects until no register holds what the exit wrote for
        // what it could not make
        let accum = self
            .unmade
            .accum
            .is_some()
            .then(|| self.trace_hide_unmade_accum(base_us, off));
        // an emergency collection keeps the registers written back (the
        // frame's pc is not yet the exit's, whose op may set no top)
        let (top, end) = (self.top, self.lua_window_end());
        self.top = top.max(end);
        self.trace_make_unmade_sunk(base_us, drop);
        if let Some(at) = accum {
            let drop = drop || self.jit.pending_raise.is_some();
            self.trace_make_unmade_accum(at, drop);
        }
        self.top = top;
        let (e, pc) = failed?;
        self.jit.pending_raise = Some(e);
        Some(pc)
    }

    /// After the registers were written back from frame `base_us`: the
    /// sunk tables the exit could not make, made now (an emergency
    /// collection and one more try each, then the memory error, raised
    /// where the interpreter goes on), into every register that holds
    /// one. `drop`: an error is raised already, and they are left nil.
    fn trace_make_unmade_sunk(&mut self, base_us: usize, drop: bool) {
        if self.unmade.roots.is_empty() {
            return;
        }
        // each register holding a stand-in becomes nil before anything
        // collects, the register and its table noted after the contents
        let w0 = self.unmade.roots.len();
        for i in base_us..self.stack.len() {
            if let Value::Table(t) = self.stack[i]
                && t.as_ptr() as usize & 1 == 1
            {
                self.stack[i] = Value::Nil;
                let at = (t.as_ptr() as usize >> 1) as i64;
                let roots = &mut self.unmade.roots;
                push_root(roots, Value::Int(i as i64));
                push_root(roots, Value::Int(at));
            }
        }
        let mut at = 0;
        while !drop && at < w0 && self.jit.pending_raise.is_none() {
            self.unmade.made.sunk += 1;
            match self.retry_oom(|vm| vm.unmade_sunk_table(at)) {
                Ok(t) => self.put_unmade_sunk(w0, at, Value::Table(t)),
                Err(e) => self.jit.pending_raise = Some(e),
            }
            at += unmade_sunk_len(&self.unmade.roots, at);
        }
        self.unmade.roots.clear();
    }

    /// Writes `t` to each register noted (past `w0`) as holding the sunk
    /// table at `at`.
    fn put_unmade_sunk(&mut self, w0: usize, at: usize, t: Value) {
        let roots = &self.unmade.roots;
        for p in roots[w0..].as_chunks::<2>().0 {
            if let [Value::Int(slot), Value::Int(of)] = *p
                && of as usize == at
            {
                self.stack[slot as usize] = t;
            }
        }
    }

    /// The sunk table whose contents start at `at` of the roots, made and
    /// filled as its `NewTable` and the stores the trace sank would have.
    /// When PUC's was counted this round, it is made without counting and
    /// stands for that one (the record is taken once: a collection before
    /// a retry may free it).
    fn unmade_sunk_table(&mut self, at: usize) -> Result<Gc<Table>, Oom> {
        let roots = &self.unmade.roots;
        let [
            Value::Int(ops),
            Value::Int(level),
            Value::Int(cap),
            Value::Int(n_hash),
        ] = *<&[Value; 4]>::try_from(&roots[at..at + 4]).expect("a sunk table's header")
        else {
            unreachable!("a sunk table's header holds integers")
        };
        let site = ((ops >> 32) as usize).wrapping_sub(1);
        let record = self
            .jit
            .ghosts
            .get_mut(level as usize)
            .and_then(|g| g.sunk.get_mut(site))
            .and_then(Option::take);
        let off = record.map(|_| self.heap.counting_off());
        let made = self.fill_unmade_sunk(ops, at + 4, cap as usize, n_hash as usize);
        if let Some(off) = off {
            self.heap.counting_on(off);
        }
        if let (Some(g), Ok(t)) = (record, &made) {
            self.heap.ghost_table_made(g, t.hdr());
        }
        made
    }

    fn fill_unmade_sunk(
        &mut self,
        ops: i64,
        from: usize,
        cap: usize,
        n_hash: usize,
    ) -> Result<Gc<Table>, Oom> {
        let (b, c, k) = (
            (ops & 0xFF) as u32,
            ((ops >> 8) & 0xFF) as u32,
            ops >> 16 & 1 != 0,
        );
        let t = self
            .heap
            .try_new_table_presized(b, c, k)
            .map_err(table_oom)?
            .expect("a sunk table's size is within a table's limits");
        for i in 0..cap {
            let v = self.unmade.roots[from + i];
            t.set_int(&mut self.heap, i as i64 + 1, v)
                .map_err(table_oom)?;
        }
        for i in 0..n_hash {
            let kv = from + cap + 2 * i;
            let (key, v) = (self.unmade.roots[kv], self.unmade.roots[kv + 1]);
            t.set(&mut self.heap, key, v).map_err(table_oom)?;
        }
        Ok(t)
    }
}

/// The words of the sunk table at `at` of `roots`.
fn unmade_sunk_len(roots: &[Value], at: usize) -> usize {
    match (roots[at + 2], roots[at + 3]) {
        (Value::Int(cap), Value::Int(n_hash)) => sunk_words(cap as usize, n_hash as usize),
        _ => unreachable!("a sunk table's header holds integers"),
    }
}

/// A fresh table's stores fail only for memory: the keys are its array
/// indices and interned strings.
fn table_oom(e: TableError) -> Oom {
    match e {
        TableError::Memory(o) => o,
        _ => unreachable!("a sunk table's store fails only for memory"),
    }
}
