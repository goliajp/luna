//! `luaG_typeerror`: the message naming an operand of the wrong type.

use super::*;

impl Vm {
    pub(crate) fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        let extra = self.subject_varinfo(v);
        let tn = self.obj_typename(v);
        let conv = self.type_err_conv(&extra);
        let e = self.typeerror_msg(what, &tn, &extra, conv);
        self.varinfo_pushed = self.version() >= LuaVersion::Lua53 && !extra.is_empty();
        e
    }

    /// PUC `luaG_typeerror`'s message for operand `extra` (luna's
    /// `" (kind 'name')"` or nothing) of type `tn`: 5.3 on, the string of
    /// `varinfo` first, then `"attempt to %s a %s value%s"`; before, the
    /// named form `"attempt to %s %s '%s' (a %s value)"` or the bare one.
    pub(in crate::vm::exec) fn typeerror_msg(
        &mut self,
        what: &str,
        tn: &str,
        extra: &str,
        conv: u32,
    ) -> LuaError {
        use crate::runtime::puc_fmt::FmtArg::S;
        let named = split_varinfo(extra);
        if self.version() > LuaVersion::Lua52 {
            if let Some((kind, name)) = named {
                let r = self
                    .heap
                    .pushfstring(b" (%s '%s')", &[S(kind.as_bytes()), S(name.as_bytes())]);
                if let Err(e) = r {
                    return e.into();
                }
            }
            let args = [S(what.as_bytes()), S(tn.as_bytes()), S(extra.as_bytes())];
            return self.runerror_fmt(
                b"attempt to %s a %s value%s",
                &args,
                conv,
                !extra.is_empty(),
            );
        }
        match named {
            Some((kind, name)) if kind != "metamethod" => {
                let args = [
                    S(what.as_bytes()),
                    S(kind.as_bytes()),
                    S(name.as_bytes()),
                    S(tn.as_bytes()),
                ];
                self.runerror_fmt(b"attempt to %s %s '%s' (a %s value)", &args, conv, true)
            }
            _ => {
                let args = [S(what.as_bytes()), S(tn.as_bytes())];
                self.runerror_fmt(b"attempt to %s a %s value", &args, conv, false)
            }
        }
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
}

/// The kind and name in luna's varinfo `" (kind 'name')"`.
pub(in crate::vm::exec) fn split_varinfo(extra: &str) -> Option<(&str, &str)> {
    let inner = extra.strip_prefix(" (")?.strip_suffix("')")?;
    inner.split_once(" '")
}
