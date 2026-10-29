//! Ending a buffer: its string pushed (`luaL_pushresult`), or the buffer
//! left on the stack as its C function returns.

use super::*;

impl PucBuffer {
    /// `luaL_pushresult`: the string of `out`.
    #[inline(always)]
    pub(crate) fn finish<const PRE: bool>(
        self,
        vm: &mut Vm,
        out: &[u8],
    ) -> Result<Gc<LuaStr>, LuaError> {
        match self.d {
            // every byte still in the C buffer: `emptybuffer` pushes them as
            // the one string, which `lua_concat(L, 1)` leaves as it is
            Dialect::L51 if self.n > 0 && vm.lib_roots.len() == self.base.at() => {
                vm.api_check_before::<true>()?;
                vm.alloc_str(&out[self.start..self.start + self.n])
            }
            Dialect::L51 => self.finish_slow::<true>(vm, out),
            _ if !self.boxed => {
                vm.api_check_before::<PRE>()?;
                let r = vm.alloc_str(out)?;
                vm.api_check_after::<PRE>(Value::Str(r))?;
                Ok(r)
            }
            _ => self.finish_slow::<PRE>(vm, out),
        }
    }

    /// [`PucBuffer::finish`] of a buffer never made (see
    /// [`PucBuffer::begun`]): all of `out` in its first block.
    #[inline(always)]
    pub(crate) fn finish_first<const PRE: bool>(
        vm: &mut Vm,
        out: &[u8],
    ) -> Result<Gc<LuaStr>, LuaError> {
        if PRE && vm.heap.dialect() == Dialect::L51 {
            // an empty 5.1 buffer pushes nothing (`emptybuffer`), and
            // `luaL_pushresult` its own empty string
            if !out.is_empty() {
                vm.api_check_before::<true>()?;
            }
            return vm.alloc_str(out);
        }
        vm.api_check_before::<PRE>()?;
        let r = vm.alloc_str(out)?;
        vm.api_check_after::<PRE>(Value::Str(r))?;
        Ok(r)
    }

    #[cold]
    #[inline(never)]
    fn finish_slow<const PRE: bool>(
        mut self,
        vm: &mut Vm,
        out: &[u8],
    ) -> Result<Gc<LuaStr>, LuaError> {
        match self.d {
            Dialect::L51 => {
                self.empty_51(vm, out)?;
                let n = vm.lib_roots.len() - self.base.at();
                let r = if n == 0 {
                    vm.alloc_str(b"")?
                } else {
                    if n >= 2 {
                        self.concat_51(vm, n)?;
                    }
                    match vm.lib_roots[self.base.at()] {
                        Value::Str(s) => s,
                        _ => unreachable!("the buffer's strings"),
                    }
                };
                vm.lib_roots.release(self.base);
                Ok(r)
            }
            Dialect::L55 if self.boxed => {
                // the box's block becomes the string's
                if self.size != out.len() + 1 {
                    vm.heap.trace_raw(self.size, out.len() + 1);
                }
                let r = vm.retry_oom(|vm| vm.heap.new_external_string(out, false))?;
                vm.api_check_after::<false>(Value::Str(r))?;
                vm.box_closed_over(r)?;
                // `lua_closeslot` leaves nil where the box was: `lua_gc(L,
                // LUA_GCSTEP, len)` finds the string on the stack, not the
                // box
                self.drop_box(vm);
                vm.lib_roots.push(Value::Str(r));
                // the box's slot and the string are above the arguments
                vm.native_push_if_native(2);
                vm.gc_top = vm.native_top().unwrap_or_else(|| vm.lua_window_end());
                let stepped = crate::runtime::puc::step::gc_lua_step(vm, out.len() as i64);
                if vm.native_on_top() {
                    vm.native_pop(2);
                }
                vm.lib_roots.pop();
                stepped?;
                Ok(r)
            }
            _ => {
                vm.api_check_before::<PRE>()?;
                let r = vm.alloc_str(out)?;
                vm.api_check_after::<PRE>(Value::Str(r))?;
                if self.boxed && self.d >= Dialect::L53 {
                    if self.d >= Dialect::L54 {
                        // `lua_closeslot` calls the box's `__close`
                        vm.box_closed_over(r)?;
                    }
                    vm.heap.trace_raw(self.size, 0);
                    self.drop_box(vm);
                }
                Ok(r)
            }
        }
    }

    /// The buffer left on the stack when its C function returns: from
    /// 5.4 on its box is a to-be-closed slot, which the return closes
    /// (`luaD_poscall`), its `__close` called above the function's top.
    pub(crate) fn close_on_return(mut self, vm: &mut Vm) -> Result<(), LuaError> {
        if self.boxed && self.d >= Dialect::L54 {
            vm.box_closed()?;
            vm.heap.trace_raw(self.size, 0);
            self.drop_box(vm);
        }
        Ok(())
    }

    fn drop_box(&mut self, vm: &mut Vm) {
        if let Some(u) = self.ubox.take() {
            vm.drop_box(u);
        }
    }
}
