//! Ending a buffer: its string pushed (`luaL_pushresult`).

use super::*;

impl PucBuffer {
    /// `luaL_pushresult`: the string of `out`.
    pub(crate) fn finish(mut self, vm: &mut Vm, out: &[u8]) -> Result<Gc<LuaStr>, LuaError> {
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
                let r = vm.retry_oom(|vm| vm.heap.new_external_string(out))?;
                vm.api_check_after(Value::Str(r))?;
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
                vm.api_check_before()?;
                let r = vm.alloc_str(out)?;
                vm.api_check_after(Value::Str(r))?;
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

    fn drop_box(&mut self, vm: &mut Vm) {
        if let Some(u) = self.ubox.take() {
            vm.drop_box(u);
        }
    }
}
