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

use super::NativeAct;

impl Vm {
    /// The running native's `L->top`, if a native is what runs.
    pub(crate) fn native_top(&self) -> Option<u32> {
        if self.native_on_top() {
            self.running_natives.last().map(NativeAct::top)
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

    /// The running native pushes `n` values (C `lua_push*`). Library code
    /// calls this from the native itself, the innermost activation; a
    /// helper that a Lua frame can reach as well uses
    /// [`Vm::native_push_if_native`].
    #[inline]
    pub(crate) fn native_push(&mut self, n: u32) {
        if let Some(a) = self.running_natives.last_mut() {
            a.top_off += n as i32;
        }
    }

    /// [`Vm::native_push`] when a native is what runs, nothing when a Lua
    /// frame is on top (`luaG_runerror` from an instruction).
    pub(crate) fn native_push_if_native(&mut self, n: u32) {
        if self.native_on_top() {
            self.native_push(n);
        }
    }

    /// The running native pops `n` values (C `lua_pop(L, n)`).
    #[inline]
    pub(crate) fn native_pop(&mut self, n: u32) {
        if let Some(a) = self.running_natives.last_mut() {
            a.top_off -= n as i32;
        }
    }

    /// The running native has `n` values pushed past its arguments (`n`
    /// below zero: it dropped that many of them), set outright before a
    /// callback or an error, after steps that counted nothing.
    #[inline]
    pub(crate) fn native_set_pushed(&mut self, n: i32) {
        if let Some(a) = self.running_natives.last_mut() {
            a.top_off = n;
        }
    }

    /// Write `vals`, values the running native's C function has pushed, to
    /// its stack from `from` slots past its arguments (below zero: over the
    /// ones it dropped): `debug.getlocal` finds them there below a function
    /// it calls, as the collector does.
    pub(crate) fn native_hold(&mut self, from: i32, vals: &[Value]) -> Result<(), LuaError> {
        // a native the interpreter runs inline has no record of its own:
        // the last one is below the running Lua function
        if !self.native_on_top() {
            return Ok(());
        }
        let Some(a) = self.running_natives.last() else {
            return Ok(());
        };
        let at = (a.func_slot + 1 + a.nargs).wrapping_add_signed(from) as usize;
        let end = at + vals.len();
        self.grow_stack_holding(end, vals)?;
        self.stack[at..end].copy_from_slice(vals);
        Ok(())
    }

    /// The running native pushes `vals` (C `lua_push*`): counted as
    /// [`Vm::native_push`] does, and written where they go.
    pub(crate) fn native_push_vals(&mut self, vals: &[Value]) -> Result<(), LuaError> {
        self.native_hold_top(vals)?;
        self.native_push(vals.len() as u32);
        Ok(())
    }

    /// [`Vm::native_hold`] at the running native's top: values a step that
    /// counts them itself pushes there.
    pub(crate) fn native_hold_top(&mut self, vals: &[Value]) -> Result<(), LuaError> {
        match self.running_natives.last() {
            Some(a) if self.native_on_top() => self.native_hold(a.top_off, vals),
            _ => Ok(()),
        }
    }

    /// What `luaL_Buffer` keeps in its stack slot (see [`Vm::buffer_slot`]):
    /// 5.4's placeholder is a light userdata, 5.2 and 5.3's box a full one,
    /// which luna stands in for with a light userdata as well.
    pub(crate) fn buffer_box() -> Value {
        static BOX: u8 = 0;
        Value::LightUserdata(&BOX as *const u8 as *const ())
    }

    /// The slots a `luaL_Buffer` holding `len` bytes takes on the stack:
    /// 5.4+ a placeholder from the start, 5.2 and 5.3 a box once the
    /// content passes `LUAL_BUFFERSIZE`, 5.1 none.
    #[inline]
    pub(crate) fn buffer_slot(&self, len: usize) -> u32 {
        match self.version() {
            LuaVersion::Lua51 => 0,
            v @ (LuaVersion::Lua52 | LuaVersion::Lua53) => u32::from(len > Vm::lual_buffersize(v)),
            _ => 1,
        }
    }

    /// The running native sets its top to `n` values (C `lua_settop(L, n)`).
    #[inline]
    pub(crate) fn native_settop(&mut self, n: u32) {
        if let Some(a) = self.running_natives.last_mut() {
            a.top_off = n as i32 - a.nargs as i32;
        }
    }

    /// [`Vm::native_settop`], the slots it adds past the top set to nil.
    pub(crate) fn native_settop_nil(&mut self, n: u32) -> Result<(), LuaError> {
        if let Some(a) = self.running_natives.last() {
            let added = (n as i32 - a.nargs as i32 - a.top_off).max(0) as usize;
            self.native_hold_top(&vec![Value::Nil; added])?;
        }
        self.native_settop(n);
        Ok(())
    }

    /// C `lua_getfield(L, idx, key)` on `obj`: the value, left pushed. 5.2+
    /// push the key first, where the value then goes, so an `__index` it
    /// runs starts one slot higher than in 5.1.
    pub(crate) fn native_getfield(&mut self, obj: Value, key: &[u8]) -> Result<Value, LuaError> {
        let k = self.str_value(key)?;
        if self.version() >= LuaVersion::Lua52 {
            self.native_push_vals(&[k])?;
            let v = self.index_value(obj, k)?;
            // the value takes the key's slot
            let top = self.running_natives.last().map_or(0, |a| a.top_off);
            self.native_hold(top - 1, &[v])?;
            return Ok(v);
        }
        let v = self.index_value(obj, k)?;
        self.native_push_vals(&[v])?;
        Ok(v)
    }

    /// C `lua_setfield(L, idx, key)` with the value `v` pushed on top: 5.2+
    /// push the key before a `__newindex` runs; both are popped after.
    pub(crate) fn native_setfield(
        &mut self,
        obj: Value,
        key: &[u8],
        v: Value,
    ) -> Result<(), LuaError> {
        let k = self.str_value(key)?;
        let key_pushed = self.version() >= LuaVersion::Lua52;
        self.native_push_vals(&[k][..usize::from(key_pushed)])?;
        self.newindex_value(obj, k, v)?;
        self.native_pop(1 + u32::from(key_pushed));
        Ok(())
    }

    /// C `luaL_buffinitsize(L, &b, n)`, or `luaL_buffinit` with `n` 0:
    /// the buffer's slot (see [`Vm::buffer_slot`]), pushed outright by a
    /// native that keeps it for the rest of its run.
    pub(crate) fn native_buffinit(&mut self, n: usize) -> Result<(), LuaError> {
        let slot = self.buffer_slot(n);
        self.native_push_vals(&[Vm::buffer_box()][..slot as usize])
    }
}

/// The checks of the stack's room `luaO_pushvfstring` makes formatting a
/// message of `conv` conversions, each as (slot, slots) counted from the
/// top it starts at: 5.1 one slot per piece it pushes (`incr_top`), 5.2
/// two before each conversion and one before the tail, 5.3 one per piece
/// (`luaD_inctop`) and the tail's twice; 5.4 on none (it counts on
/// `EXTRA_STACK`).
fn pushvfstring_checks(v: LuaVersion, conv: u32) -> Vec<(u32, u32)> {
    match v {
        LuaVersion::Lua51 => (0..2 * conv + 2).map(|i| (i, 1)).collect(),
        LuaVersion::Lua52 => (0..conv)
            .map(|i| (2 * i, 2))
            .chain([(2 * conv, 1)])
            .collect(),
        LuaVersion::Lua53 => (0..2 * conv)
            .map(|i| (i, 1))
            .chain([(2 * conv, 1), (2 * conv, 1)])
            .collect(),
        _ => Vec::new(),
    }
}

impl Vm {
    /// The pushes of a running native formatting an error message, from its
    /// top: each part a `lua_pushfstring` of that many conversions, or
    /// `None` for a value pushed as it is, leaving one value. They grow the
    /// stack as PUC's do (see `pushvfstring_checks`); a push past the
    /// limit raises "stack overflow" there instead of the message, and one
    /// past the room an overflow left "error in error handling".
    pub(crate) fn format_pushes(&mut self, parts: &[Option<u32>]) -> Option<LuaError> {
        let top = self.raise_from.or_else(|| self.native_top())?;
        self.pushes_from(top, parts, 0)
    }

    /// PUC `luaG_runerror`'s pushes formatting a message of `conv`
    /// conversions: on a native's stack (see [`Vm::format_pushes`]), or
    /// above a Lua function's whole window, after the operand's name
    /// (5.3 `varinfo`) and then the position (`luaG_addinfo`).
    pub(crate) fn runerror_pushes(&mut self, conv: u32, varinfo: bool) -> Option<LuaError> {
        if self.version() >= LuaVersion::Lua54 {
            return None;
        }
        if self.native_on_top() {
            return self.format_pushes(&[Some(conv)]);
        }
        let top = self.raise_from.unwrap_or_else(|| self.lua_window_end());
        if varinfo && self.version() == LuaVersion::Lua53 {
            self.pushes_from(top, &[Some(2), Some(conv), Some(3)], 1)
        } else {
            self.pushes_from(top, &[Some(conv), Some(3)], 1)
        }
    }

    /// The pushes of [`Vm::format_pushes`] from slot `top`; an overflow is
    /// raised with the position a Lua function's error gets (`positioned`).
    fn pushes_from(
        &mut self,
        mut top: u32,
        parts: &[Option<u32>],
        positioned: u32,
    ) -> Option<LuaError> {
        use crate::vm::exec::stack_size::{self, Grown};
        for part in parts {
            for (off, n) in part.map_or_else(Vec::new, |c| pushvfstring_checks(self.version(), c)) {
                let grew = top + off + 1 + n >= stack_size::last(self.version(), self.stack_size);
                match stack_size::check(self.version(), self.stack_size, top + off + 1, n) {
                    Grown::Fits(s) => self.regrow_stack(grew, s),
                    Grown::Overflow(s) => {
                        self.resize_stack(s);
                        self.overflow_top = Some(top + off + positioned);
                        let msg = format_args!("stack overflow");
                        return Some(if positioned == 0 {
                            self.plain_errf(msg)
                        } else {
                            self.rt_errf(msg)
                        });
                    }
                    Grown::ErrErr => return Some(LuaError(self.errerr())),
                }
            }
            top += 1;
        }
        None
    }
}
