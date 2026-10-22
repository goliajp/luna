//! Saving and loading a coroutine's execution context.

use super::*;
use crate::runtime::mem::LVec;

/// A thread's swapped-out execution context (PUC per-thread stack state).
pub(super) struct SavedCtx {
    pub(super) stack: LVec<Value>,
    pub(super) frames: LVec<CallFrame>,
    pub(super) open_upvals: LVec<(u32, Gc<Upvalue>)>,
    pub(super) tbc: LVec<u32>,
    pub(super) top: u32,
    /// the top of the native that switched the thread away (PUC's top of a
    /// suspended thread, whose slots above are dead); `u32::MAX`, the whole
    /// stack, when the host switched it with no native running
    pub(super) gc_top: u32,
    pub(super) meta_conts: u32,
    pub(super) stale_frames: u32,
    pub(super) stack_size: u32,
    /// the natives running on the thread, in `Vm::running_natives`
    pub(super) natives: std::ops::Range<usize>,
    pub(super) frame_size: u32,
    pub(super) ci_alloc: u32,
    pub(super) hook: HookState,
    /// PUC `L->l_gt` — the thread's own globals table. Carried alongside
    /// the rest of the suspended state so each thread can keep its own
    /// `setfenv(0, env)` rewire without the swap leaking into another
    /// thread (5.1 closure.lua :177).
    pub(super) globals: Gc<Table>,
}

impl SavedCtx {
    /// The thread's stack, frames and counts, back into its coroutine.
    pub(super) fn store_stack_in(self, m: &mut Coro) {
        m.stack = self.stack;
        m.frames = self.frames;
        m.open_upvals = self.open_upvals;
        m.tbc = self.tbc;
        m.top = self.top;
        m.gc_top = self.gc_top;
        m.meta_conts = self.meta_conts;
        m.stale_frames = self.stale_frames;
        m.stack_size = self.stack_size;
        m.frame_size = self.frame_size;
        m.ci_alloc = self.ci_alloc;
    }
}

impl Vm {
    /// The main thread's stack while a coroutine runs, from its top up:
    /// dead, so cleared by every collection, before it frees what the
    /// slots point to (they are not marked).
    pub(super) fn clear_saved_main(&mut self) {
        if let Some(mc) = &mut self.main_ctx {
            let lo = (mc.gc_top as usize).min(mc.stack.len());
            mc.stack[lo..].fill(Value::Nil);
        }
    }

    pub(super) fn take_ctx(&mut self) -> SavedCtx {
        // read before the frames it looks at are taken
        let gc_top = self.native_top().unwrap_or(u32::MAX);
        let saved = SavedCtx {
            stack: self.stack.take(),
            frames: self.frames.take(),
            open_upvals: self.open_upvals.take(),
            tbc: self.tbc.take(),
            top: self.top,
            gc_top,
            meta_conts: self.g.meta_conts,
            stale_frames: self.g.stale_frames,
            stack_size: self.stack_size,
            natives: self.natives_base..self.running_natives.len(),
            frame_size: self.g.frame_size,
            ci_alloc: self.g.ci_alloc,
            hook: self.hook,
            globals: self.globals,
        };
        self.frames_resync(); // frames now empty
        saved
    }

    pub(super) fn put_ctx(&mut self, c: SavedCtx) {
        self.stack = c.stack;
        self.frames = c.frames;
        self.open_upvals = c.open_upvals;
        self.tbc = c.tbc;
        self.top = c.top;
        self.g.meta_conts = c.meta_conts;
        self.g.stale_frames = c.stale_frames;
        self.set_stack_size(c.stack_size);
        self.g.frame_size = c.frame_size;
        self.g.ci_alloc = c.ci_alloc;
        self.g.ci_hint = 0;
        self.hook = c.hook;
        self.globals = c.globals;
        self.frames_resync(); // sync shadow to new Vec
    }

    /// Move a coroutine's saved context into the live VM fields.
    pub(super) fn load_coro_ctx(&mut self, co: Gc<Coro>) {
        // SAFETY: `co` is the coroutine `resume_coro` is switching to (or its resumer `r`), which the caller holds and which is a root through `self.current` or a saved stack; `m` is the only reference into it until the function returns, and nothing here can collect
        let m = unsafe { co.as_mut() };
        self.stack = m.stack.take();
        self.frames = m.frames.take();
        self.open_upvals = m.open_upvals.take();
        self.tbc = m.tbc.take();
        self.top = m.top;
        // a thread's stack starts at the basic size (`stack_init`)
        self.set_stack_size(match m.stack_size {
            0 => stack_size::initial(self.version),
            s => s,
        });
        // a thread's 5.1 frame array starts at the basic size (PUC
        // `lua_newthread`: `stack_init`)
        self.g.frame_size = match m.frame_size {
            0 if self.version == LuaVersion::Lua51 => BASIC_FRAME_SIZE_51,
            0 => u32::MAX,
            n => n,
        };
        self.g.ci_alloc = m.ci_alloc;
        self.g.ci_hint = 0;
        self.frames_resync(); // sync shadow to coro's frames
        self.g.meta_conts = m.meta_conts;
        self.g.stale_frames = m.stale_frames;
        self.hook = m.hook;
        self.globals = m.globals;
    }

    /// Save the live VM context back into a coroutine object.
    pub(super) fn store_coro_ctx(&mut self, co: Gc<Coro>) {
        let c = self.take_ctx();
        // SAFETY: `co` is the coroutine `resume_coro` is switching away from, held by its caller; `take_ctx` above did not touch it, and `m` is the only reference into it until the barrier call, which takes only its address
        let m = unsafe { co.as_mut() };
        m.hook = c.hook;
        m.globals = c.globals;
        c.store_stack_in(m);
        // suspended, its top is where the yield left it
        m.gc_top = if m.resume_at.is_some() {
            m.yield_top
        } else {
            u32::MAX
        };
        // bulk-overwrite of every collectable field traced by Coro::trace:
        // demote the coro back to gray so propagate re-traces its new state.
        self.heap.barrier_back(co);
    }
}
