//! What PUC's `pushfuncname` and `pushglobalfuncname` decide about a
//! function: where the loaded modules (5.2: the globals) hold it, and which
//! of its names a traceback shows. Nothing here makes a Lua object, so the
//! traceback a host takes after an error (made while the error is raised,
//! when PUC makes nothing) and the ones made as PUC makes them, string by
//! string, share it.

use super::Ar;
use crate::runtime::{Gc, LuaStr, Table, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

/// Where `findfield` found a function: the key in the root table, and the
/// key under it when it was found one table deeper.
#[derive(Clone, Copy)]
pub(crate) struct GlobalPath {
    pub(crate) outer: Gc<LuaStr>,
    pub(crate) inner: Option<Gc<LuaStr>>,
}

impl GlobalPath {
    /// Whether the joined name (`outer.inner`) starts with `"_G."`, which
    /// 5.3 on drop.
    pub(crate) fn under_g(&self) -> bool {
        let o = self.outer.as_bytes();
        o.starts_with(b"_G.") || (self.inner.is_some() && o == b"_G")
    }

    /// The name as `pushglobalfuncname` leaves it in dialect `v`.
    pub(crate) fn name(&self, v: LuaVersion) -> Vec<u8> {
        let mut n = self.outer.as_bytes().to_vec();
        if let Some(i) = self.inner {
            n.push(b'.');
            n.extend_from_slice(i.as_bytes());
        }
        if v >= LuaVersion::Lua53 && self.under_g() {
            n.drain(..3);
        }
        n
    }
}

/// Which name a traceback level shows for its function, `G` being how the
/// caller holds a global name.
pub(crate) enum FuncName<G> {
    /// `function '<global name>'`
    Global(G),
    /// `<word> '<name>'`, the name the caller's code gave
    Named(&'static str),
    /// `main chunk`
    Main,
    /// `function <source:line>`
    Lua,
    /// `?`
    Unknown,
}

impl Vm {
    /// PUC `findfield(L, objidx, 2)` from the loaded modules (5.2: the
    /// global table): the first string-keyed path to `f`, in the tables'
    /// traversal order.
    pub(crate) fn global_path(&self, f: Value) -> Option<GlobalPath> {
        let root = if self.version() == LuaVersion::Lua52 {
            self.globals()
        } else {
            self.loaded_modules()?
        };
        find_path(root, f)
    }

    /// `pushfuncname` of 5.2 to 5.5 for level `ar`: which name it shows.
    /// `global` looks the function's global name up, when the dialect's
    /// order asks for it.
    pub(crate) fn func_name<G, E>(
        &mut self,
        ar: &Ar,
        mut global: impl FnMut(&mut Vm) -> Result<Option<G>, E>,
    ) -> Result<FuncName<G>, E> {
        let v = self.version();
        if matches!(v, LuaVersion::Lua53 | LuaVersion::Lua54)
            && let Some(g) = global(self)?
        {
            return Ok(FuncName::Global(g));
        }
        if let Some((what, _)) = ar.name.as_ref().filter(|(what, _)| !what.is_empty()) {
            return Ok(FuncName::Named(if v == LuaVersion::Lua52 {
                "function"
            } else {
                what
            }));
        }
        if ar.what == "main" {
            return Ok(FuncName::Main);
        }
        let g = match v {
            LuaVersion::Lua52 if ar.what == "C" => global(self)?,
            LuaVersion::Lua55 => global(self)?,
            _ => None,
        };
        Ok(match g {
            Some(g) => FuncName::Global(g),
            None if ar.what != "C" => FuncName::Lua,
            None => FuncName::Unknown,
        })
    }
}

fn find_path(t: Gc<Table>, f: Value) -> Option<GlobalPath> {
    let mut k = Value::Nil;
    while let Ok(Some((nk, nv))) = t.next(k) {
        k = nk;
        let Value::Str(outer) = nk else { continue };
        if nv.raw_eq(f) {
            return Some(GlobalPath { outer, inner: None });
        }
        if let Value::Table(inner_t) = nv {
            let mut k2 = Value::Nil;
            while let Ok(Some((nk2, nv2))) = inner_t.next(k2) {
                k2 = nk2;
                if let Value::Str(inner) = nk2
                    && nv2.raw_eq(f)
                {
                    return Some(GlobalPath {
                        outer,
                        inner: Some(inner),
                    });
                }
            }
        }
    }
    None
}
