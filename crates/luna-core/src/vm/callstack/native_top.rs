//! The stack top of the running native, as PUC's C function has it.
//!
//! A luna native keeps its working values in Rust, but where PUC's C
//! function has its `L->top` decides two things a script can see: the slot
//! a message handler runs at when the native raises (`luaG_errormsg` puts
//! it where the error object was), and the slot a function the native calls
//! back starts at (a comparator, a metamethod, `tostring`). So the natives
//! and the error helpers they share follow what the C code pushes and pops.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Vm {
    /// The running native's `L->top`, if a native is what runs.
    pub(crate) fn native_top(&self) -> Option<u32> {
        if self.native_on_top() {
            self.running_natives.last().map(|a| a.top)
        } else {
            None
        }
    }

    /// Where the window of the Lua function on top of the frames ends (PUC
    /// `ci->top`), or the stack's end when no Lua function runs.
    pub(crate) fn lua_window_end(&self) -> u32 {
        match self.frames.last() {
            Some(crate::runtime::function::CallFrame::Lua(f)) => {
                f.base + u32::from(f.closure.proto.max_stack)
            }
            _ => self.stack.len() as u32,
        }
    }

    /// The running native pushes `n` values (C `lua_push*`).
    pub(crate) fn native_push(&mut self, n: u32) {
        if self.native_on_top()
            && let Some(a) = self.running_natives.last_mut()
        {
            a.top += n;
        }
    }

    /// The running native pops `n` values (C `lua_pop(L, n)`).
    pub(crate) fn native_pop(&mut self, n: u32) {
        if self.native_on_top()
            && let Some(a) = self.running_natives.last_mut()
        {
            a.top -= n;
        }
    }

    /// The running native sets its top to `n` values (C `lua_settop(L, n)`).
    pub(crate) fn native_settop(&mut self, n: u32) {
        if self.native_on_top()
            && let Some(a) = self.running_natives.last_mut()
        {
            a.top = a.func_slot + 1 + n;
        }
    }

    /// C `lua_getfield(L, idx, key)` on `obj`: the value, left pushed. 5.2+
    /// push the key first, where the value then goes, so an `__index` it
    /// runs starts one slot higher than in 5.1.
    pub(crate) fn native_getfield(&mut self, obj: Value, key: &[u8]) -> Result<Value, LuaError> {
        let k = Value::Str(self.heap.intern(key));
        let key_first = self.version() >= LuaVersion::Lua52;
        self.native_push(u32::from(key_first));
        let v = self.index_value(obj, k)?;
        self.native_push(u32::from(!key_first));
        Ok(v)
    }

    /// C `luaL_buffinitsize(L, &b, n)`, or `luaL_buffinit` with `n` 0:
    /// 5.4+ push a placeholder for the buffer, 5.2 and 5.3 a box only once
    /// the content passes `LUAL_BUFFERSIZE` (8192 there), 5.1 nothing.
    /// Whether the buffer now has a slot, for `native_buffgrown`.
    pub(crate) fn native_buffinit(&mut self, n: usize) -> bool {
        let pushed = match self.version() {
            LuaVersion::Lua51 => false,
            LuaVersion::Lua52 | LuaVersion::Lua53 => n > LUAL_BUFFERSIZE_52,
            _ => true,
        };
        self.native_push(u32::from(pushed));
        pushed || self.version() == LuaVersion::Lua51
    }

    /// The buffer holds `len` bytes: 5.2 and 5.3 push its box the first
    /// time that passes `LUAL_BUFFERSIZE`.
    pub(crate) fn native_buffgrown(&mut self, slotted: &mut bool, len: usize) {
        if !*slotted && len > LUAL_BUFFERSIZE_52 {
            *slotted = true;
            self.native_push(1);
        }
    }
}

/// 5.2's and 5.3's `LUAL_BUFFERSIZE` (`BUFSIZ`, 8192 with glibc)
const LUAL_BUFFERSIZE_52: usize = 8192;
