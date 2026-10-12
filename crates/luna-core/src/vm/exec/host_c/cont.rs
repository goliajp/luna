//! The continuations a host C function leaves when it calls or yields with one.

use super::*;

impl Vm {
    pub(super) fn push_host_cont(
        &mut self,
        spec: HostContSpec,
        results_at: u32,
    ) -> Result<(), LuaError> {
        self.push_cf(CallFrame::Cont(NativeCont {
            kind: ContKind::Host(HostCont {
                results_at,
                token: spec.token,
            }),
            func_slot: spec.func_slot,
            nresults: spec.nresults,
        }))
    }

    /// A [`ContKind::Host`] frame came to the top (already popped): its call
    /// returned, or its coroutine was resumed. Run the continuation with the
    /// C function back among the running natives, and finish the C
    /// function's call with what the continuation returns.
    pub(crate) fn finish_host_cont(
        &mut self,
        nc: NativeCont,
        hc: HostCont,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        let at = hc.results_at as usize;
        let top = (self.top as usize).max(at);
        // `top` may run ahead of the stack's length
        self.grow_stack(top)?;
        let vals = self.stack[at..top].to_vec();
        self.stack.truncate(at);
        self.top = hc.results_at;
        let hooks = self
            .host_cont_hooks
            .expect("a C continuation without the C API");
        let Value::Native(ncl) = self.stack[nc.func_slot as usize] else {
            unreachable!("a C continuation's slot holds its C function")
        };
        let act = NativeAct::new(ncl, nc.func_slot, 0, self.frames.len(), 0);
        self.running_natives.push(act)?;
        self.native_nresults = nc.nresults;
        let thread = self.host_thread();
        match self.in_native_window(|vm| (hooks.resume)(vm, thread, hc.token, nc.func_slot, vals)) {
            Ok(nret) => {
                if self.frames.len() < entry_depth {
                    self.running_natives.pop();
                    self.top = nc.func_slot + nret;
                    return Ok(Some(self.take_results(nc.func_slot)));
                }
                self.finish_native_call(nc.func_slot, 0, nret, nc.nresults)?;
                Ok(None)
            }
            Err(e) => {
                let act = self.running_natives.pop().expect("pushed above");
                if self.yielding.is_none() {
                    self.note_errored_native(act, e.0);
                }
                Err(e)
            }
        }
    }

    /// An error dropped a [`ContKind::Host`] frame.
    pub(crate) fn discard_host_cont(&mut self, hc: HostCont) {
        if let Some(hooks) = self.host_cont_hooks {
            let thread = self.host_thread();
            (hooks.discard)(self, thread, hc.token);
        }
    }
}
