//! A call C makes into Lua, above the values C keeps on its own stack.
//! PUC has one stack: what a C function (or the host, on the base level)
//! pushed lies below the function it calls, and the stack grows and
//! shrinks with it there. luna keeps those values on the C API's stack, so
//! the call is placed where PUC's would start.

use super::*;

impl Vm {
    /// Call `f(args)` from C (PUC `lua_callk`), `below` the values C has on
    /// its stack under `f`, from its frame's base (`None`: a call C does
    /// not make from its stack, as a hook's or a `__close`'s). With `cont`
    /// and a thread that can yield, a yield inside `f` suspends the
    /// coroutine with the continuation on the frame stack: the `Err` that
    /// comes back is then the yield (`Vm::host_yielding`), and the C
    /// function must leave. Without `cont` the call cannot yield.
    pub(crate) fn host_call(
        &mut self,
        f: Value,
        args: &[Value],
        cont: Option<HostContSpec>,
        below: Option<u32>,
    ) -> Result<Vec<Value>, LuaError> {
        let held = below.and_then(|n| self.c_values_below(n));
        let Some(spec) = cont.filter(|_| self.yield_barrier().is_none()) else {
            let r = self.call_value(f, args);
            self.c_values_released(held);
            return r;
        };
        let results_at = self.stack.len() as u32;
        self.push_host_cont(spec, results_at)?;
        let r = self.call_value_k(f, args);
        if r.is_err() && self.yielding.is_some() {
            return r;
        }
        frames_pop_sync(&mut self.frames, &mut self.frames_top, &mut self.trap);
        r
    }

    /// PUC `lua_checkstack(L, n)` with `pushed` values on the running C
    /// function's frame (on the host's level, on the stack): where PUC's
    /// stack would hold them, it grows as PUC's does, and the frame
    /// reserves the room.
    pub(crate) fn host_c_checkstack(&mut self, pushed: u32, n: i64) -> bool {
        let top = match self.running_natives.last() {
            Some(a) if self.native_on_top() => a.func_slot + 1 + pushed,
            _ => pushed,
        };
        self.checkstack(top, n)
    }

    /// `n` values C has under the function it calls: the running C
    /// function has pushed them past its arguments (or dropped some of
    /// those), which its top counts; on the host's level, where only the
    /// main thread runs and nothing yields, they are held on the stack for
    /// the call: where the stack ended before.
    fn c_values_below(&mut self, n: u32) -> Option<usize> {
        if self.native_on_top() {
            let nargs = self.running_natives.last().map_or(0, |a| a.nargs);
            self.native_set_pushed(n as i32 - nargs as i32);
            return None;
        }
        let len = self.stack.len();
        for _ in 0..n {
            self.stack.push_or_abort(Value::Nil);
        }
        self.top = self.stack.len() as u32;
        Some(len)
    }

    /// The slots [`Vm::c_values_below`] held on the host's level, from
    /// `len`, given back.
    fn c_values_released(&mut self, len: Option<usize>) {
        if let Some(len) = len {
            self.stack.truncate(len);
            self.top = len as u32;
        }
    }
}
