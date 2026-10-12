//! `luaG_typeerror`: the message naming an operand of the wrong type.

use super::*;

impl Vm {
    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        let extra = self.subject_varinfo(v);
        let tn = self.obj_typename(v);
        let msg = self.compose_type_err(what, &tn, &extra);
        let conv = self.type_err_conv(&extra);
        self.runerror_named(&msg, &extra, conv)
    }

    /// The conversions of PUC's `luaG_typeerror` format for an operand
    /// named by `extra`: 5.3 `"attempt to %s a %s value%s"`; before, the
    /// named form `"attempt to %s %s '%s' (a %s value)"` or the bare one.
    pub(in crate::vm::exec) fn type_err_conv(&self, extra: &str) -> u32 {
        if self.version() > LuaVersion::Lua52 {
            return 3;
        }
        let inner = extra.trim_start().trim_start_matches('(');
        if inner.is_empty() || inner.starts_with("metamethod") {
            2
        } else {
            4
        }
    }

    /// [`Self::runerror`] for a message of `conv` conversions that names an
    /// operand with `extra` (see `varinfo_pushed`).
    pub(in crate::vm::exec) fn runerror_named(
        &mut self,
        msg: &str,
        extra: &str,
        conv: u32,
    ) -> LuaError {
        let e = self.runerror_conv(msg, conv, !extra.is_empty());
        self.varinfo_pushed = self.version() >= LuaVersion::Lua53 && !extra.is_empty();
        e
    }

    /// Assemble a `luaG_typeerror` / `luaG_callerror` message in the dialect's
    /// word order.
    ///
    /// PUC ≤5.2 names the operand first — `attempt to call field 'f' (a nil
    /// value)`. 5.3 flipped it to type-first — `attempt to call a nil value
    /// (field 'f')`. luna emitted the 5.3+ form on every dialect, so every
    /// such error was worded wrong under 5.1/5.2.
    ///
    /// Two shapes carry no operand name on ≤5.2 and must collapse to the bare
    /// message: an absent varinfo (identical across dialects), and a
    /// metamethod target — ≤5.2's `luaG_typeerror` only names locals, globals,
    /// fields, upvalues and methods, so `(metamethod 'add')` has no ≤5.2
    /// counterpart and is dropped rather than reworded. All four shapes were
    /// measured against stock 5.1.5 / 5.2.4 / 5.5.1 before this was written.
    pub(in crate::vm::exec) fn compose_type_err(
        &self,
        what: &str,
        tn: &str,
        extra: &str,
    ) -> String {
        if self.version() > crate::version::LuaVersion::Lua52 {
            return format!("attempt to {what} a {tn} value{extra}");
        }
        // `extra` is "" or " (kind 'name')" — unwrap to "kind 'name'".
        let inner = extra
            .trim_start()
            .trim_start_matches('(')
            .trim_end_matches(')');
        if inner.is_empty() || inner.starts_with("metamethod") {
            format!("attempt to {what} a {tn} value")
        } else {
            format!("attempt to {what} {inner} (a {tn} value)")
        }
    }
}
