//! The call and return hooks of `pcall` and `xpcall`, C functions of
//! PUC's that luna runs under a continuation.

use super::*;

impl Vm {
    /// `pcall` / `xpcall` (`nc`) at `func_slot`, called from Lua with
    /// `nargs` arguments: the call hook PUC's `luaD_precall` runs before
    /// the C function, which is level 1 in it.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn protected_call_hook(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        ccmt: u8,
    ) -> Result<(), LuaError> {
        let depth = self.frames.len();
        self.push_native(crate::vm::callstack::NativeAct::new(
            nc, func_slot, nargs, depth, ccmt,
        ))?;
        let r = self.hook_call(true, nargs);
        self.running_natives.pop();
        self.g.ci_hint += 1;
        r
    }

    /// The return hook of the `pcall` / `xpcall` of continuation `nc` (one
    /// Lua called), its `n` results at `first` where PUC's C function
    /// leaves them, before they move to its slot: the native is level 1
    /// again, its window up to them (PUC `rethook`).
    pub(in crate::vm::exec) fn protected_return_hook(
        &mut self,
        nc: NativeCont,
        first: u32,
        n: u32,
    ) -> Result<(), LuaError> {
        let Value::Native(f) = self.stack[nc.func_slot as usize] else {
            return Ok(());
        };
        let fs = nc.func_slot;
        let depth = self.frames.len();
        let mut act = crate::vm::callstack::NativeAct::new(f, fs, first + n - fs - 1, depth, 0);
        act.raise_ci_top(0);
        self.push_native(act)?;
        let r = self.hook_return(true, first - fs, n);
        self.running_natives.pop();
        self.g.ci_hint += 1;
        r
    }

    /// The `pcall` / `xpcall` of `nc`, if Lua called it, returned the `nret`
    /// results at `callee`: its return hook, the results with `true`
    /// below them as PUC's C function returns them; the results are back
    /// at `callee` after it.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn protected_returned(
        &mut self,
        nc: NativeCont,
        callee: u32,
        nret: u32,
    ) -> Result<(), LuaError> {
        if nc.kind.ci_top() == 0 || !self.ret_hook_armed() {
            return Ok(());
        }
        // 5.1's `pcall` puts `true` where its function was
        let at = if callee == nc.func_slot + 1 {
            callee + 1
        } else {
            callee
        };
        let (from, to, n) = (callee as usize, at as usize, nret as usize);
        self.grow_stack(to + n)?;
        self.stack.copy_within(from..from + n, to);
        self.stack[to - 1] = Value::Bool(true);
        let r = self.protected_return_hook(nc, at - 1, nret + 1);
        self.stack.copy_within(to..to + n, from);
        r
    }

    /// The `pcall` / `xpcall` of `nc` (one Lua called) caught `err`: its
    /// return hook, `false` and the error where PUC's C function returns
    /// them. 5.1's `pcall` puts them where its function was; its `xpcall`
    /// and 5.2's put `false` one slot lower, over what they keep there;
    /// from 5.3 they go above the error `luaD_pcall` left where the
    /// function was, the `true` pushed on entry below it.
    #[cold]
    #[inline(never)]
    pub(in crate::vm::exec) fn protected_caught(
        &mut self,
        nc: NativeCont,
        err: Value,
    ) -> Result<(), LuaError> {
        let callee = (i64::from(nc.func_slot) + 1 + i64::from(nc.kind.callee_shift())) as u32;
        let first = match (self.version, nc.kind) {
            (LuaVersion::Lua51, ContKind::Pcall { .. }) => callee,
            (LuaVersion::Lua51 | LuaVersion::Lua52, _) => callee - 1,
            _ => callee + 1,
        };
        self.grow_stack((first + 2) as usize)?;
        if first > callee {
            self.stack[callee as usize - 1] = Value::Bool(true);
        }
        self.stack[callee as usize] = err;
        self.stack[first as usize] = Value::Bool(false);
        self.stack[first as usize + 1] = err;
        self.protected_return_hook(nc, first, 2)
    }

    /// A return hook would fire now.
    #[inline(always)]
    pub(in crate::vm::exec) fn ret_hook_armed(&self) -> bool {
        self.hook.ret && self.hook_armed()
    }
}
