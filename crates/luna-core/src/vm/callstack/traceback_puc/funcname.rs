//! `pushfuncname` and `pushglobalfuncname` as PUC's traceback makes them:
//! each string it makes on the way, and the loaded modules searched for
//! the function's name.

use super::*;

impl Vm {
    /// `pushfuncname` of 5.2 to 5.5: the string, made with what PUC makes
    /// on the way, each checked for as PUC checks.
    pub(super) fn puc_funcname(&mut self, ar: &Ar) -> Result<Gc<LuaStr>, LuaError> {
        let base = self.lib_roots.len();
        let r = self.funcname_pushed(ar);
        let s = self.lib_roots.last().copied();
        self.lib_roots.truncate(base);
        r?;
        match s {
            Some(Value::Str(s)) => Ok(s),
            _ => unreachable!("pushfuncname pushes a string"),
        }
    }

    pub(super) fn funcname_pushed(&mut self, ar: &Ar) -> Result<(), LuaError> {
        let v = self.version();
        let named = ar.name.as_ref().filter(|(what, _)| !what.is_empty());
        let global = |vm: &mut Vm| -> Result<Option<Gc<LuaStr>>, LuaError> {
            let g = vm.puc_global_name(ar.func)?;
            if let Some(g) = g {
                vm.lib_roots.push(Value::Str(g));
            }
            Ok(g)
        };
        if matches!(v, LuaVersion::Lua53 | LuaVersion::Lua54)
            && let Some(g) = global(self)?
        {
            return self.api_push(|h| h.pushfstring(b"function '%s'", &[FmtArg::S(g.as_bytes())]));
        }
        if let Some((what, n)) = named {
            return if v == LuaVersion::Lua52 {
                self.api_push(|h| h.pushfstring(b"function '%s'", &[FmtArg::S(n.as_bytes())]))
            } else {
                let args = [FmtArg::S(what.as_bytes()), FmtArg::S(n.as_bytes())];
                self.api_push(|h| h.pushfstring(b"%s '%s'", &args))
            };
        }
        if ar.what == "main" {
            return self.api_push(|h| h.try_intern(b"main chunk"));
        }
        let g = match v {
            LuaVersion::Lua52 if ar.what == "C" => global(self)?,
            LuaVersion::Lua55 => global(self)?,
            _ => None,
        };
        if let Some(g) = g {
            return self.api_push(|h| h.pushfstring(b"function '%s'", &[FmtArg::S(g.as_bytes())]));
        }
        if ar.what != "C" {
            let args = [FmtArg::S(&ar.short_src), FmtArg::D(ar.linedefined)];
            return self.api_push(|h| h.pushfstring(b"function <%s:%d>", &args));
        }
        self.api_push(|h| h.try_intern(b"?"))
    }

    /// `pushglobalfuncname`: the name the loaded modules (5.2: the
    /// globals) hold `f` under, made as PUC makes it.
    pub(super) fn puc_global_name(&mut self, f: Value) -> Result<Option<Gc<LuaStr>>, LuaError> {
        let root = if self.version() == LuaVersion::Lua52 {
            Some(self.globals())
        } else {
            self.loaded_table()
        };
        let Some(root) = root else {
            return Ok(None);
        };
        let Some(name) = self.findfield(root, f, 2)? else {
            return Ok(None);
        };
        if self.version() >= LuaVersion::Lua53
            && let Some(rest) = name.as_bytes().strip_prefix(b"_G.")
        {
            let rest = rest.to_vec();
            self.lib_roots.push(Value::Str(name));
            let r = self.api_push(|h| h.try_intern(&rest));
            let s = self.lib_roots.pop();
            self.lib_roots.pop();
            r?;
            return Ok(match s {
                Some(Value::Str(s)) => Some(s),
                _ => None,
            });
        }
        Ok(Some(name))
    }

    /// `package.loaded`, PUC's `_LOADED`.
    pub(super) fn loaded_table(&mut self) -> Option<Gc<Table>> {
        let pkg = Value::Str(self.heap.find_interned(b"package")?);
        let Value::Table(p) = self.globals().get(pkg) else {
            return None;
        };
        let loaded = Value::Str(self.heap.find_interned(b"loaded")?);
        match p.get(loaded) {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    /// `findfield`: a string-keyed path to `f` at most `level` tables deep,
    /// in traversal order; a path through a table is joined with `"."`.
    pub(super) fn findfield(
        &mut self,
        t: Gc<Table>,
        f: Value,
        level: u32,
    ) -> Result<Option<Gc<LuaStr>>, LuaError> {
        if level == 0 {
            return Ok(None);
        }
        let mut k = Value::Nil;
        while let Ok(Some((nk, nv))) = t.next(k) {
            k = nk;
            let Value::Str(ks) = nk else { continue };
            if nv.raw_eq(f) {
                return Ok(Some(ks));
            }
            if let Value::Table(inner) = nv
                && let Some(name) = self.findfield(inner, f, level - 1)?
            {
                // the module's name, ".", the field's, joined
                let base = self.lib_roots.len();
                self.lib_roots.push(nk);
                let r = self.api_push(|h| h.try_intern(b".")).and_then(|()| {
                    self.lib_roots.push(Value::Str(name));
                    self.api_concat(base)
                });
                let s = self.lib_roots.get(base).copied();
                self.lib_roots.truncate(base);
                r?;
                if let Some(Value::Str(s)) = s {
                    return Ok(Some(s));
                }
            }
        }
        Ok(None)
    }
}
