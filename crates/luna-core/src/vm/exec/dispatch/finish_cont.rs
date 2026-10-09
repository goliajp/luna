//! Finishing a continuation frame when the call it protected delivered.

use super::*;

impl Vm {
    /// A continuation frame is on top: the call it protected has delivered
    /// its results (or a `__close` handler / yieldable metamethod finished).
    /// `Some` hands results out of this activation.
    #[inline(never)]
    pub(crate) fn finish_cont(
        &mut self,
        nc: NativeCont,
        entry_depth: usize,
    ) -> Result<Option<Vec<Value>>, LuaError> {
        // a yieldable metamethod returned: complete the interrupted
        // instruction (PUC luaV_finishOp) and resume the running frame.
        if let ContKind::Meta(mc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            self.cont_popped(true, true);
            let result = if self.top > nc.func_slot {
                self.stack[nc.func_slot as usize]
            } else {
                Value::Nil
            };
            self.stack.truncate(mc.saved_len as usize);
            self.top = mc.saved_top;
            self.finish_meta(mc.action, result)?;
            return Ok(None);
        }
        // a __close handler returned successfully: discard its
        // results, restore `top` to the slot the handler was called
        // at (the surrounding frame's register window above this slot
        // must stay alloc'd — never truncate the underlying stack),
        // then continue the close chain (next slot, or fire
        // AfterClose). When the close ends an entry activation,
        // drive_close hands the results up to exec_with directly.
        if let ContKind::Close(cc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            self.cont_popped(true, true);
            let pending = cc.has_pending.then(|| self.stack[nc.func_slot as usize]);
            self.top = nc.func_slot;
            if let Some(vals) = self.drive_close(cc.from, pending, cc.after, entry_depth)? {
                return Ok(Some(vals));
            }
            return Ok(None);
        }
        // __pairs returned: normalize its results to exactly the
        // dialect's count (iterator, state, control, and on 5.5 the
        // closing value) at pairs's slot, where the metamethod was
        // called, and hand them to pairs's caller.
        if let ContKind::Pairs { at } = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            self.cont_popped(true, true);
            let total = crate::vm::builtins::pairs_mm_results(self) as u32;
            let need = (nc.func_slot + total) as usize;
            if self.stack.len() < need {
                self.grow_stack_or_abort(need);
            }
            // where the metamethod ran, above pairs's arguments
            let first = nc.func_slot + at;
            let n = (self.top - first).min(total);
            for i in 0..n {
                self.stack[(nc.func_slot + i) as usize] = self.stack[(first + i) as usize];
            }
            for s in (nc.func_slot + n)..(nc.func_slot + total) {
                self.stack[s as usize] = Value::Nil;
            }
            self.top = nc.func_slot + total;
            if self.frames.len() < entry_depth {
                return Ok(Some(self.take_results(nc.func_slot)));
            }
            self.finish_results(nc.func_slot, total, nc.nresults);
            return Ok(None);
        }
        if let ContKind::Host(hc) = nc.kind {
            frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
            return self.finish_host_cont(nc, hc, entry_depth);
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        self.cont_popped(false, nc.kind.is_level());
        // f's results sit where f was called (`callee_shift` above the
        // slot after the continuation): moved to that slot, writing `true`
        // at the continuation's makes `true, results…` contiguous
        let callee = (i64::from(nc.func_slot) + 1 + i64::from(nc.kind.callee_shift())) as u32;
        let nret = self.top - callee;
        if callee != nc.func_slot + 1 {
            // the results may end past the stack's length (`top` runs ahead
            // of it); both ranges of the move must be inside it
            let end = (callee.max(nc.func_slot + 1) + nret) as usize;
            if self.stack.len() < end {
                self.grow_stack_or_abort(end);
            }
            self.stack.copy_within(
                callee as usize..(callee + nret) as usize,
                (nc.func_slot + 1) as usize,
            );
        }
        self.stack[nc.func_slot as usize] = Value::Bool(true);
        let total = 1 + nret;
        self.top = nc.func_slot + total;
        if self.frames.len() < entry_depth {
            return Ok(Some(self.take_results(nc.func_slot)));
        }
        self.finish_results(nc.func_slot, total, nc.nresults);
        Ok(None)
    }
}
