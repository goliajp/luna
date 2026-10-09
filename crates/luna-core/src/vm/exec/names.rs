//! Names the debug API reports: placeholder local names and the global
//! name of a function.

use super::*;

impl Vm {
    /// PUC's debug-API placeholder for an unnamed vararg slot returned by
    /// `debug.getlocal(_, -n)`. 5.2/5.3 spelled it `"(*vararg)"`; 5.4
    /// dropped the asterisk in favour of `"(vararg)"`. db.lua 5.2 :189 /
    /// 5.3 :195 / 5.4 :286 baseline on their respective form.
    pub(crate) fn vararg_locvar_name(&self) -> &'static str {
        if matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            "(*vararg)"
        } else {
            "(vararg)"
        }
    }

    /// PUC's debug-API placeholder for an unnamed temporary on a C
    /// activation. 5.2/5.3 reported `"(*temporary)"`; 5.4 switched to
    /// `"(C temporary)"`. db.lua 5.2 :288, 5.3 :312, 5.4 :404 each pin
    /// their spelling.
    pub(crate) fn temporary_locvar_name(&self) -> &'static str {
        if matches!(
            self.version,
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53
        ) {
            // PUC 5.1's `findlocal` C-frame branch reported `(*temporary)`
            // (db.lua :228 pins it). 5.2/5.3 kept the spelling, 5.4 changed
            // to `(C temporary)`.
            "(*temporary)"
        } else {
            "(C temporary)"
        }
    }

    /// PUC's debug-API placeholder for an unnamed Lua-frame temporary
    /// (an arithmetic intermediate sitting past the last named local on a
    /// live register slot). 5.2/5.3 reported `"(*temporary)"`; 5.4 dropped
    /// the asterisk to `"(temporary)"`. db.lua 5.3 :786, 5.4 :966 pin the
    /// spelling.
    pub(crate) fn lua_temporary_locvar_name(&self) -> &'static str {
        if matches!(
            self.version,
            LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua53
        ) {
            "(*temporary)"
        } else {
            "(temporary)"
        }
    }

    /// PUC `pushglobalfuncname`: walk `package.loaded` to depth 2 looking for
    /// `target` itself (`lua_rawequal`), and return its qualified
    /// name (e.g. `"table.sort"`). A `_G.X` match is stripped to `"X"`. Returns
    /// `None` if no match is found. Used by `arg_error` when the running native
    /// was invoked from another native (PUC `ar.name == NULL` at level 0).
    pub(crate) fn pushglobalfuncname(
        &mut self,
        target: Gc<crate::runtime::NativeClosure>,
    ) -> Option<String> {
        let pkg_k = Value::Str(self.heap.intern(b"package"));
        let pkg = match self.globals().get(pkg_k) {
            Value::Table(t) => t,
            _ => return None,
        };
        let loaded_k = Value::Str(self.heap.intern(b"loaded"));
        let loaded = match pkg.get(loaded_k) {
            Value::Table(t) => t,
            _ => return None,
        };
        let matches = |v: Value| -> bool { matches!(v, Value::Native(nc) if nc.ptr_eq(target)) };
        let mut k = Value::Nil;
        while let Ok(Some((nk, nv))) = loaded.next(k) {
            k = nk;
            let Value::Str(outer) = nk else { continue };
            let outer = String::from_utf8_lossy(outer.as_bytes()).into_owned();
            if matches(nv) {
                return Some(if outer == "_G" { String::new() } else { outer });
            }
            if let Value::Table(inner_t) = nv {
                let mut k2 = Value::Nil;
                while let Ok(Some((nk2, nv2))) = inner_t.next(k2) {
                    k2 = nk2;
                    if matches(nv2)
                        && let Value::Str(inner) = nk2
                    {
                        let inner = String::from_utf8_lossy(inner.as_bytes()).into_owned();
                        return Some(if outer == "_G" {
                            inner
                        } else {
                            format!("{outer}.{inner}")
                        });
                    }
                }
            }
        }
        None
    }

    /// How the caller named the running native (PUC `lua_getinfo("n")` at
    /// level 0): `None` when it gives no name, as when the caller is C.
    pub(crate) fn running_call_name(&self) -> Option<(&'static str, String)> {
        let ts = self.thread_stack(None);
        if ts.levels.is_empty() {
            return None;
        }
        self.level_name(&ts, 0)
    }

    /// Read an upvalue cell of a closure (debug.getupvalue).
    pub(crate) fn upvalue_value(&self, cl: Gc<LuaClosure>, idx: usize) -> Value {
        match cl.upvals()[idx].state() {
            UpvalState::Open { slot, thread } => self.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        }
    }

    /// Write an upvalue cell of a closure (debug.setupvalue).
    pub(crate) fn upvalue_set_value(&mut self, cl: Gc<LuaClosure>, idx: usize, v: Value) {
        let uv = cl.upvals()[idx];
        match uv.state() {
            UpvalState::Open { slot, thread } => self.write_slot(slot, thread, v),
            UpvalState::Closed(_) => {
                // SAFETY: `uv` is an upvalue of `cl`, a closure the caller holds (a debug.setupvalue argument); the `state()` copy has ended, so no reference into the cell is live, and the borrow covers one call
                unsafe { uv.as_mut() }.set_closed(v);
                self.heap.barrier_forward(uv, v);
            }
        }
    }
}
