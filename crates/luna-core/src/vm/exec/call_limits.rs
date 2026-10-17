//! The Lua stack and frame array limits of a call.

use super::stack_size::{self, Grown};
use super::*;

impl Vm {
    /// Room for `need` slots above `top` on the thread's stack (PUC
    /// `luaD_checkstack` at a call, `top` being `L->top` there), the stack
    /// grown as PUC grows it, else the error PUC raises. `gc`: 5.3 and 5.4
    /// check for a collection before they grow it (`checkstackGCp`).
    #[inline(always)]
    pub(crate) fn check_lua_stack(
        &mut self,
        top: u32,
        need: u32,
        gc: bool,
    ) -> Result<(), LuaError> {
        if top + need < self.g.stack_last {
            return Ok(());
        }
        self.grow_lua_stack(top, need, gc)
    }

    /// The running thread's stack is now `s` slots (PUC's count).
    #[inline]
    pub(crate) fn set_stack_size(&mut self, s: u32) {
        self.stack_size = s;
        self.g.stack_last = fast_stack_last(self.version, s);
    }

    /// The running thread's stack resized by PUC to `s` slots
    /// (`luaD_reallocstack`), as opposed to a switch of threads.
    pub(crate) fn resize_stack(&mut self, s: u32) {
        self.heap.acct_stack(self.stack_size, s);
        self.set_stack_size(s);
    }

    /// The running thread's 5.1 `CallInfo` array resized to `n` entries
    /// (`luaD_reallocCI`).
    pub(super) fn resize_frames_51(&mut self, n: u32) {
        self.heap.acct_ci51(self.g.frame_size, n);
        self.g.frame_size = n;
    }

    /// PUC `luaD_checkstack` past the fast bound: the stack grows, or past
    /// the limit the first overflow raises "stack overflow" and leaves the
    /// stack the room for the message handler that runs on it; a call that
    /// does not fit that room either is "error in error handling"
    /// (errors.lua :606, cstack.lua :29).
    #[cold]
    #[inline(never)]
    pub(super) fn grow_lua_stack(&mut self, top: u32, need: u32, gc: bool) -> Result<(), LuaError> {
        if gc
            && matches!(self.version, LuaVersion::Lua53 | LuaVersion::Lua54)
            && top + 1 + need >= stack_size::last(self.version, self.stack_size)
        {
            self.checkpoint(top)?;
        }
        if self.version == LuaVersion::Lua51 && top + need > STACK_CAP_51 {
            return Err(self.rt_err("stack overflow"));
        }
        match stack_size::check(self.version, self.stack_size, top + 1, need) {
            Grown::Fits(s) => {
                self.resize_stack(s);
                Ok(())
            }
            Grown::Overflow(s) => {
                self.resize_stack(s);
                // before 5.4 `luaG_runerror` adds the position to the
                // message of a Lua function, pushing it too; a C function
                // (one whose message handler is being called on a full
                // stack) gets none
                let positioned = self.version() < LuaVersion::Lua54 && !self.native_on_top();
                self.overflow_top = Some(top + u32::from(positioned));
                Err(self.rt_err("stack overflow"))
            }
            Grown::ErrErr => Err(LuaError(self.errerr())),
        }
    }

    /// Room for one more frame. 5.1 checks its call limit here, as PUC
    /// 5.1's `luaD_growCI` does when its `CallInfo` array is full: an array
    /// already past `LUAI_MAXCALLS` (grown for a message handler) is "error
    /// in error handling"; otherwise it doubles, and when that takes it
    /// past the limit the call raises "stack overflow". The frame array's
    /// capacity plays the `CallInfo` array's size; calls compiled code made
    /// natively (`frames_native`) count as frames too.
    #[cold]
    #[inline(never)]
    pub(super) fn grow_frames(&mut self) -> Result<(), LuaError> {
        if self.g.frame_size > self.frame_cap {
            return Err(LuaError(self.errerr()));
        }
        self.resize_frames_51(self.g.frame_size * 2);
        if self.g.frame_size > self.frame_cap {
            return Err(self.rt_err("stack overflow"));
        }
        Ok(())
    }

    /// The frames PUC 5.1 has in its `CallInfo` array for the running
    /// thread: the Lua frames and the protected calls on the frame stack,
    /// the native functions running on the Rust stack, and the calls
    /// compiled code made natively.
    pub(super) fn frames_in_use(&self) -> u32 {
        let natives = (self.running_natives.len() - self.natives_base) as u32;
        // a coroutine's array starts with its base frame; the main thread's
        // is the host's call, with the host's own frames below it
        let base = if self.current.is_some() {
            1
        } else {
            self.g.host_frames
        };
        self.frames.len() as u32 + natives + self.g.frames_native - self.g.meta_conts + base
    }

    /// PUC 5.1 `restore_stack_limit`, after a protected call caught an
    /// error: a frame array grown past the limit for a message handler goes
    /// back to the limit, unless the frames still in use come near it.
    pub(super) fn restore_frame_limit(&mut self) {
        if self.g.frame_size > self.frame_cap && self.frames_in_use() + 1 < self.frame_cap {
            self.resize_frames_51(self.frame_cap);
        }
    }
}
