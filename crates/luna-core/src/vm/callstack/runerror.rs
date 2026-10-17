//! PUC's `luaG_runerror`: a message with the position of the running Lua
//! function.

use crate::vm::exec::Vm;

impl Vm {
    /// PUC `luaG_runerror`: `msg` with the position of the running function
    /// when that is a Lua function; a running native adds none.
    pub(crate) fn runerror(&mut self, msg: &str) -> crate::vm::error::LuaError {
        self.runerror_conv(msg, 0, false)
    }

    /// [`Vm::runerror`] for a message PUC formats with `conv` conversions
    /// (see [`Vm::rt_errf_conv`]).
    pub(crate) fn runerror_conv(
        &mut self,
        msg: &str,
        conv: u32,
        varinfo: bool,
    ) -> crate::vm::error::LuaError {
        if self.native_on_top() {
            if let Some(e) = self.format_pushes(&[Some(conv)]) {
                return e;
            }
            if let Err(e) = self.runerror_checkpoint() {
                return e;
            }
            self.plain_err(msg)
        } else {
            self.rt_errf_conv(format_args!("{msg}"), conv, varinfo)
        }
    }

    /// [`Vm::runerror_conv`] for PUC's format `fmt` and its `args`, made
    /// as PUC makes the message.
    pub(crate) fn runerror_fmt(
        &mut self,
        fmt: &[u8],
        args: &[crate::runtime::puc_fmt::FmtArg<'_>],
        conv: u32,
        varinfo: bool,
    ) -> crate::vm::error::LuaError {
        let pos = if self.native_on_top() {
            if let Some(e) = self.format_pushes(&[Some(conv)]) {
                return e;
            }
            if let Err(e) = self.runerror_checkpoint() {
                return e;
            }
            None
        } else {
            if let Some(e) = self.runerror_pushes(conv, varinfo) {
                return e;
            }
            if let Err(e) = self.runerror_checkpoint() {
                return e;
            }
            self.varinfo_pushed = false;
            self.error_position()
        };
        self.native_push_if_native(1);
        self.raise_msg(pos, fmt, args)
    }

    /// 5.3 on, `luaG_runerror` checks for a collection before it makes the
    /// message: the stack is live up to the running function's top (a Lua
    /// one's whole frame, with the operand's name `varinfo` pushed).
    pub(crate) fn runerror_checkpoint(&mut self) -> Result<(), crate::vm::error::LuaError> {
        if self.version() < crate::version::LuaVersion::Lua53 {
            return Ok(());
        }
        let top = match self.native_top() {
            Some(t) => t,
            None => self.lua_window_end() + u32::from(self.varinfo_pushed),
        };
        self.checkpoint(top)
    }
}
