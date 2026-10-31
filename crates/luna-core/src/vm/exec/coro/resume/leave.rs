//! Back from a resumed coroutine to the thread that resumed it.

use super::*;

/// What switching to a coroutine replaced, given back when it stops.
pub(super) struct Resumer {
    pub(super) nccalls: u32,
    pub(super) special_before: SpecialErrors,
    pub(super) resumer: Option<Gc<Coro>>,
    pub(super) natives_base: usize,
    pub(super) msgh_floor: usize,
    pub(super) msgh_running: Option<Value>,
    pub(super) keeps_traceback: bool,
    pub(super) nny: u32,
}

impl Vm {
    /// `co` stopped with `drive`: what the resume returns, after `co` keeps
    /// its context and the resumer gets back its own.
    #[cfg_attr(luna_hot, inline(always))]
    pub(super) fn resume_leave(
        &mut self,
        co: Gc<Coro>,
        saved: Resumer,
        drive: Result<Vec<Value>, LuaError>,
    ) -> Result<Vec<Value>, LuaError> {
        let Resumer {
            nccalls,
            special_before,
            resumer,
            natives_base,
            msgh_floor,
            msgh_running,
            keeps_traceback,
            nny,
        } = saved;
        // classify: a self-close termination or a pending yield each win over
        // the (sentinel) error they raised to unwind the Rust stack.
        let (outcome, status) = if let Some(death) = self.terminating.take() {
            // the coroutine closed itself: it dies now, cleanly or with the
            // error a `__close` handler raised.
            let r = match death {
                Some(e) => {
                    self.set_coro_error(co, e, crate::runtime::ErrorStatus::Run);
                    (Err(LuaError(e)), CoroStatus::Dead)
                }
                None => (Ok(Vec::new()), CoroStatus::Dead),
            };
            self.host_thread_reset(co);
            r
        } else {
            match self.yielding.take() {
                Some((vals, fslot, nres)) => {
                    // SAFETY: `co` is still `self.current`, a root; the borrow covers two field stores
                    let c = unsafe { co.as_mut() };
                    c.resume_at = Some((fslot, nres));
                    c.yield_ci_top = std::mem::take(&mut self.yield_ci_top);
                    (Ok(vals), CoroStatus::Suspended)
                }
                None => {
                    // died: a return is clean, an error is remembered so a later
                    // `coroutine.close` can report it (PUC lua_closethread).
                    // The frames taken where the error was raised stay with
                    // the dead coroutine, as PUC's untouched dead stack does
                    // (db.lua :848 family)
                    if drive.is_err() {
                        let dead = self.dead_snapshot.take();
                        // SAFETY: `co` is still `self.current`, a root, and its frames have unwound, so `m` is the only reference into it for this store
                        unsafe { co.as_mut() }.dead = dead;
                        self.heap.barrier_back(co);
                    }
                    if let Err(e) = drive {
                        // a hook that failed in it is over with it
                        self.hooks_caught(0);
                        let kind = self.error_status(e.0, special_before);
                        self.set_coro_error(co, e.0, kind);
                    }
                    (self.host_returned(co, drive), CoroStatus::Dead)
                }
            }
        };

        // save the coroutine's context back and restore the resumer
        self.natives_base = natives_base;
        self.msgh_floor = msgh_floor;
        self.msgh_running = msgh_running;
        self.keep_error_traceback = keeps_traceback;
        self.nny = nny;
        self.store_coro_ctx(co);
        self.set_coro_status(co, status);
        match resumer {
            Some(r) => {
                self.load_coro_ctx(r);
                self.set_coro_status(r, CoroStatus::Running);
                self.current = Some(r);
            }
            None => {
                let m = self.main_ctx.take().expect("main context saved");
                self.put_ctx(m);
                self.current = None;
            }
        }
        self.g.nccalls = nccalls;
        outcome
    }
}
