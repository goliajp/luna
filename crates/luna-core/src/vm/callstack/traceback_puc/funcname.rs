//! `pushfuncname` and `pushglobalfuncname` as PUC's traceback makes them:
//! each string it makes on the way, and the loaded modules searched for
//! the function's name.

use super::*;
use crate::runtime::puc::api::PucText;
use crate::vm::callstack::func_name::FuncName;

impl Vm {
    /// `pushfuncname` of 5.2 to 5.5: the string, made with what PUC makes
    /// on the way, each checked for as PUC checks.
    pub(super) fn puc_funcname(&mut self, ar: &Ar) -> Result<Gc<LuaStr>, LuaError> {
        let base = self.lib_roots.mark();
        let r = self.funcname_pushed(ar);
        let s = self.lib_roots.last().copied();
        self.lib_roots.release(base);
        r?;
        match s {
            Some(Value::Str(s)) => Ok(s),
            _ => unreachable!("pushfuncname pushes a string"),
        }
    }

    pub(super) fn funcname_pushed(&mut self, ar: &Ar) -> Result<(), LuaError> {
        let global = |vm: &mut Vm| -> Result<Option<Gc<LuaStr>>, LuaError> {
            let g = vm.puc_global_name(ar.func)?;
            if let Some(g) = g {
                vm.lib_roots.push(Value::Str(g));
            }
            Ok(g)
        };
        match self.func_name(ar, global)? {
            FuncName::Global(g) => {
                self.api_push(|h| h.pushfstring(b"function '%s'", &[FmtArg::S(g.as_bytes())]))
            }
            FuncName::Named(word) => {
                let n = ar.name.as_ref().map_or("", |(_, n)| n.as_str());
                if self.version() == LuaVersion::Lua52 {
                    self.api_push(|h| h.pushfstring(b"function '%s'", &[FmtArg::S(n.as_bytes())]))
                } else {
                    let args = [FmtArg::S(word.as_bytes()), FmtArg::S(n.as_bytes())];
                    self.api_push(|h| h.pushfstring(b"%s '%s'", &args))
                }
            }
            FuncName::Main => self.api_push(|h| h.try_intern(b"main chunk")),
            FuncName::Lua => {
                let args = [FmtArg::S(&ar.short_src), FmtArg::D(ar.linedefined)];
                self.api_push(|h| h.pushfstring(b"function <%s:%d>", &args))
            }
            FuncName::Unknown => self.api_push(|h| h.try_intern(b"?")),
        }
    }

    /// `pushglobalfuncname`: the name the loaded modules (5.2: the
    /// globals) hold `f` under, made as PUC makes it: a field of a module
    /// joins the module's name, `"."` and the field's (`findfield`), and
    /// 5.3 on push the name again without a leading `"_G."`.
    pub(super) fn puc_global_name(&mut self, f: Value) -> Result<Option<Gc<LuaStr>>, LuaError> {
        let Some(path) = self.global_path(f) else {
            return Ok(None);
        };
        let name = match path.inner {
            None => path.outer,
            Some(inner) => {
                let base = self.lib_roots.mark();
                self.lib_roots.push(Value::Str(path.outer));
                let r = self.api_push(|h| h.try_intern(b".")).and_then(|()| {
                    self.lib_roots.push(Value::Str(inner));
                    self.api_concat(base.at())
                });
                let s = self.lib_roots.get(base.at()).copied();
                self.lib_roots.release(base);
                r?;
                match s {
                    Some(Value::Str(s)) => s,
                    _ => unreachable!("a concatenation of strings"),
                }
            }
        };
        if self.version() >= LuaVersion::Lua53 && path.under_g() {
            let rest = name.as_bytes()[3..].to_vec();
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
}
