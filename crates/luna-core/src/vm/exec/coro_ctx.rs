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
    pub(super) meta_conts: u32,
    pub(super) stale_frames: u32,
    pub(super) stack_extra: bool,
    pub(super) frame_size: u32,
    pub(super) hook: HookState,
    /// PUC `L->l_gt` — the thread's own globals table. Carried alongside
    /// the rest of the suspended state so each thread can keep its own
    /// `setfenv(0, env)` rewire without the swap leaking into another
    /// thread (5.1 closure.lua :177).
    pub(super) globals: Gc<Table>,
}

impl Vm {
    pub(super) fn take_ctx(&mut self) -> SavedCtx {
        let saved = SavedCtx {
            stack: self.stack.take(),
            frames: self.frames.take(),
            open_upvals: self.open_upvals.take(),
            tbc: self.tbc.take(),
            top: self.top,
            meta_conts: self.g.meta_conts,
            stale_frames: self.g.stale_frames,
            stack_extra: self.stack_extra,
            frame_size: self.g.frame_size,
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
        self.stack_extra = c.stack_extra;
        self.g.frame_size = c.frame_size;
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
        self.stack_extra = m.stack_extra;
        // a thread's 5.1 frame array starts at the basic size (PUC
        // `lua_newthread`: `stack_init`)
        self.g.frame_size = match m.frame_size {
            0 if self.version == LuaVersion::Lua51 => BASIC_FRAME_SIZE_51,
            0 => u32::MAX,
            n => n,
        };
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
        m.stack = c.stack;
        m.frames = c.frames;
        m.open_upvals = c.open_upvals;
        m.tbc = c.tbc;
        m.top = c.top;
        m.meta_conts = c.meta_conts;
        m.stale_frames = c.stale_frames;
        m.stack_extra = c.stack_extra;
        m.frame_size = c.frame_size;
        m.hook = c.hook;
        m.globals = c.globals;
        // bulk-overwrite of every collectable field traced by Coro::trace:
        // demote the coro back to gray so propagate re-traces its new state.
        self.heap.barrier_back(co);
    }
}
