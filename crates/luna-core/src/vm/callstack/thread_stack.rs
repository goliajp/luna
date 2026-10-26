//! The call stack of a thread: the running one, a suspended or normal
//! coroutine, or one an error killed.

use crate::runtime::function::{CallFrame, ContKind};
use crate::runtime::{Coro, CoroStatus, Gc};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

use super::ThreadStack;

impl Vm {
    /// The call stack of `co`, or of the running thread for `None` (or when
    /// `co` is the running coroutine).
    pub(crate) fn thread_stack(&self, co: Option<Gc<Coro>>) -> ThreadStack<'_> {
        let v51 = self.version() <= LuaVersion::Lua51;
        match co {
            Some(co) if self.is_main_coro(co) && self.current.is_some() => self.main_thread_stack(),
            // a coroutine an error killed: its frames as they were then
            Some(co) if co.status == CoroStatus::Dead && co.dead.is_some() => {
                // SAFETY: `co` stays allocated while the view lives (the
                // caller holds it as a native argument, and no collect runs
                // while the view borrows `self`); a dead coroutine is not
                // written while `&self` is held
                let c: &Coro = unsafe { &*co.as_ptr() };
                let d = c.dead.as_ref().expect("checked above");
                ThreadStack::new(v51, &d.frames, &c.stack, c.top, &d.natives, None)
            }
            Some(co) if !self.is_current_thread(Some(co)) && !self.is_main_coro(co) => {
                // SAFETY: `co` stays allocated while the view lives (the
                // caller holds it as a native argument, and no collect runs
                // while the view borrows `self`); it is not the running
                // thread, and nothing writes a non-running coroutine while
                // `&self` is held, so the shared reference is not aliased
                self.waiting_coro_stack(unsafe { &*co.as_ptr() }, v51)
            }
            _ => ThreadStack::new(
                v51,
                &self.frames,
                &self.stack,
                self.top,
                &self.running_natives[self.natives_base..],
                None,
            ),
        }
    }

    /// The stack of `c`, a coroutine that is not running: suspended, or
    /// normal (inside the natives that resumed another).
    fn waiting_coro_stack<'a>(&'a self, c: &'a Coro, v51: bool) -> ThreadStack<'a> {
        // a C function that yielded with a continuation is the level
        // itself (its `ContKind::Host` frame), not a `coroutine.yield`
        let parked_in_c = matches!(
            c.frames.last(),
            Some(CallFrame::Cont(nc)) if matches!(nc.kind, ContKind::Host(_))
        );
        let yield_slot = match c.status {
            CoroStatus::Suspended if !parked_in_c => c
                .resume_at
                .map(|(fs, _)| fs)
                .filter(|&fs| fs != crate::vm::exec::HOOK_YIELD_SLOT),
            _ => None,
        };
        // a normal coroutine is inside the natives that resumed another (at
        // least `coroutine.resume` itself)
        let natives = match c.status {
            CoroStatus::Normal => c.natives.clone(),
            _ => 0..0,
        };
        let ts = ThreadStack::new(
            v51,
            &c.frames,
            &c.stack,
            c.top,
            &self.running_natives[natives],
            yield_slot,
        );
        if c.status == CoroStatus::Normal {
            ts.suspended()
        } else {
            ts
        }
    }
}
