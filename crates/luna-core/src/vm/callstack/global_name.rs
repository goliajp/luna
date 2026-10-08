//! PUC `pushglobalfuncname`: naming a function by where it is stored.

use crate::runtime::{Gc, Table, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

impl Vm {
    /// PUC `pushglobalfuncname`: the name `f` is reachable by, two tables
    /// deep, from the loaded modules (5.3+, dropping a `_G.` prefix) or the
    /// global table (5.2, where PUC's `_G.` spelling depends on hash order
    /// and luna keeps the short form).
    pub(crate) fn global_func_name(&mut self, f: Value) -> Option<String> {
        let root = if self.version() == LuaVersion::Lua52 {
            self.globals()
        } else {
            let pkg_k = Value::Str(self.heap.intern(b"package"));
            let Value::Table(pkg) = self.globals().get(pkg_k) else {
                return None;
            };
            let loaded_k = Value::Str(self.heap.intern(b"loaded"));
            let Value::Table(loaded) = pkg.get(loaded_k) else {
                return None;
            };
            loaded
        };
        let name = find_field(root, f, 2)?;
        Some(match name.strip_prefix("_G.") {
            Some(rest) => rest.to_string(),
            None => name,
        })
    }
}

/// PUC `findfield`: a string-keyed path to `f` at most `level` tables deep,
/// in the table's traversal order.
fn find_field(t: Gc<Table>, f: Value, level: u32) -> Option<String> {
    if level == 0 {
        return None;
    }
    let mut k = Value::Nil;
    while let Ok(Some((nk, nv))) = t.next(k) {
        k = nk;
        let Value::Str(key) = nk else { continue };
        let key = String::from_utf8_lossy(key.as_bytes());
        if nv.raw_eq(f) {
            return Some(key.into_owned());
        }
        if let Value::Table(inner) = nv
            && let Some(rest) = find_field(inner, f, level - 1)
        {
            return Some(format!("{key}.{rest}"));
        }
    }
    None
}
