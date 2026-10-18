//! The auxiliary library calls of the script (`lauxlib.c`), made of the
//! C API calls they make, in their order: so opening a library again
//! does what PUC's does (5.1 fills the table it finds, 5.3 on leave a
//! loaded library as it is).

use super::run::{Ix, Opener};
use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// `lauxlib.h`'s names of the registry's tables.
const LOADED: &[u8] = b"_LOADED";

impl Opener {
    /// `luaL_setfuncs` (5.2 on): each function of `list` made with the
    /// `nup` values on top as upvalues, stored in the table below them;
    /// a placeholder (no function) gets `false`.
    pub(crate) fn setfuncs(
        &mut self,
        vm: &mut Vm,
        list: &[(u16, u32)],
        nup: u32,
    ) -> Result<(), LuaError> {
        for &(name, f) in list {
            let name = self.s.strs[usize::from(name)];
            if self.skipped(name) {
                continue;
            }
            if f == 0 {
                self.push(vm, Value::Bool(false))?;
            } else {
                for _ in 0..nup {
                    self.pushvalue(vm, -i64::from(nup))?;
                }
                self.pushcclosure(vm, f, nup)?;
            }
            let ix = self.ix(vm, -i64::from(nup) - 2);
            self.setfield(vm, ix, name)?;
        }
        self.pop(vm, nup);
        Ok(())
    }

    /// `dofile` and `loadfile`, which the base library `Vm::open_base`
    /// opens leaves out.
    fn skipped(&self, name: &[u8]) -> bool {
        self.sandbox && (name == b"dofile" || name == b"loadfile")
    }

    /// 5.1's `luaL_register` (`luaI_openlib` without upvalues): with a
    /// name, into the table `package.loaded` or the global of that name
    /// holds, else a new one stored in both; without, into the table on
    /// top.
    pub(crate) fn register(
        &mut self,
        vm: &mut Vm,
        libname: Option<&[u8]>,
        list: &[(u16, u32)],
    ) -> Result<(), LuaError> {
        if let Some(name) = libname {
            self.findtable(vm, Ix::Registry, LOADED, 1)?;
            let ix = self.ix(vm, -1);
            self.getfield(vm, ix, name)?;
            if !matches!(self.get(vm, -1), Value::Table(_)) {
                self.pop(vm, 1);
                if self.findtable(vm, Ix::Globals, name, list.len())?.is_some() {
                    let msg = format!(
                        "name conflict for module '{}'",
                        String::from_utf8_lossy(name)
                    );
                    return Err(LuaError(vm.str_value(msg.as_bytes())?));
                }
                self.pushvalue(vm, -1)?;
                let ix = self.ix(vm, -3);
                self.setfield(vm, ix, name)?;
            }
            self.remove(vm, -2);
        }
        self.methods = libname.is_none();
        for &(name, f) in list {
            let name = self.s.strs[usize::from(name)];
            if self.skipped(name) {
                continue;
            }
            let r = self.pushcclosure(vm, f, 0);
            let r = r.and_then(|()| {
                let ix = self.ix(vm, -2);
                self.setfield(vm, ix, name)
            });
            if r.is_err() {
                self.methods = false;
                return r;
            }
        }
        self.methods = false;
        Ok(())
    }

    /// 5.1's `luaL_findtable`: the table at the dotted path `fname` from
    /// `ix` pushed, each missing one made (the last with room for
    /// `szhint` fields); the offset of the part whose value is not a
    /// table, with nothing pushed.
    pub(crate) fn findtable(
        &mut self,
        vm: &mut Vm,
        ix: Ix,
        fname: &[u8],
        szhint: usize,
    ) -> Result<Option<usize>, LuaError> {
        let v = self.at(vm, ix);
        self.push(vm, v)?;
        let mut start = 0;
        loop {
            let end = fname[start..]
                .iter()
                .position(|&b| b == b'.')
                .map_or(fname.len(), |p| start + p);
            let part = &fname[start..end];
            let more = end < fname.len();
            self.pushstring(vm, part)?;
            let ix = self.ix(vm, -2);
            self.rawget(vm, ix);
            match self.get(vm, -1) {
                Value::Nil => {
                    self.pop(vm, 1);
                    self.createtable(vm, 0, if more { 1 } else { szhint })?;
                    self.pushstring(vm, part)?;
                    self.pushvalue(vm, -2)?;
                    let ix = self.ix(vm, -4);
                    self.settable(vm, ix)?;
                }
                Value::Table(_) => {}
                _ => {
                    self.pop(vm, 2);
                    return Ok(Some(start));
                }
            }
            self.remove(vm, -2);
            if !more {
                return Ok(None);
            }
            start = end + 1;
        }
    }

    /// `luaL_getsubtable` (5.2 on): the table `ix[fname]` pushed, made
    /// and stored there when it is not one; whether it was there.
    pub(crate) fn getsubtable(
        &mut self,
        vm: &mut Vm,
        ix: Ix,
        fname: &[u8],
    ) -> Result<bool, LuaError> {
        self.getfield(vm, ix, fname)?;
        if matches!(self.get(vm, -1), Value::Table(_)) {
            return Ok(true);
        }
        self.pop(vm, 1);
        self.createtable(vm, 0, 0)?;
        self.pushvalue(vm, -1)?;
        self.setfield(vm, ix, fname)?;
        Ok(false)
    }

    /// `luaL_newmetatable`: the registry's table `tname` pushed, made
    /// when there is none (from 5.3 on with `__name`); whether it was
    /// made. The files' metatable is luna's `file_mt` too.
    pub(crate) fn newmetatable(&mut self, vm: &mut Vm, tname: &[u8]) -> Result<bool, LuaError> {
        self.getfield(vm, Ix::Registry, tname)?;
        let made = self.get(vm, -1).is_nil();
        if made {
            self.pop(vm, 1);
            if self.v >= LuaVersion::Lua53 {
                self.createtable(vm, 0, 2)?;
                self.pushstring(vm, tname)?;
                let ix = self.ix(vm, -2);
                self.setfield(vm, ix, b"__name")?;
            } else {
                self.createtable(vm, 0, 0)?;
            }
            self.pushvalue(vm, -1)?;
            self.setfield(vm, Ix::Registry, tname)?;
        }
        if tname == b"FILE*"
            && let Value::Table(t) = self.get(vm, -1)
        {
            vm.file_mt = Some(t);
        }
        Ok(made)
    }

    /// `luaL_requiref` (5.2 on): the library opened by the function the
    /// script numbers `f`, unless (5.3 on) `package.loaded` holds it,
    /// stored there and (`glb`) as the global `modname`, left on top.
    pub(crate) fn requiref(
        &mut self,
        vm: &mut Vm,
        modname: &[u8],
        f: u32,
        glb: bool,
    ) -> Result<(), LuaError> {
        if self.v == LuaVersion::Lua52 {
            self.pushcclosure(vm, f, 0)?;
            self.pushstring(vm, modname)?;
            self.call(vm, 1, 1)?;
            self.getsubtable(vm, Ix::Registry, LOADED)?;
            self.pushvalue(vm, -2)?;
            let ix = self.ix(vm, -2);
            self.setfield(vm, ix, modname)?;
            self.pop(vm, 1);
        } else {
            self.getsubtable(vm, Ix::Registry, LOADED)?;
            let ix = self.ix(vm, -1);
            self.getfield(vm, ix, modname)?;
            if !self.get(vm, -1).truthy() {
                self.pop(vm, 1);
                self.pushcclosure(vm, f, 0)?;
                self.pushstring(vm, modname)?;
                self.call(vm, 1, 1)?;
                self.pushvalue(vm, -1)?;
                let ix = self.ix(vm, -3);
                self.setfield(vm, ix, modname)?;
            }
            self.remove(vm, -2);
        }
        if glb {
            self.pushvalue(vm, -1)?;
            self.setglobal(vm, modname)?;
        }
        Ok(())
    }

    /// Library `name` (`_G` for the base library) opened alone, as a host
    /// opens it: `luaL_requiref(L, name, luaopen_*, 1)` and the library
    /// popped; in 5.1 a call of `luaopen_*` with the name (the base
    /// library's empty). Nothing when the dialect has no such library.
    pub(crate) fn open_one(&mut self, vm: &mut Vm, name: &str) -> Result<(), LuaError> {
        let Some(f) = super::opener_of(self.s, name) else {
            return Ok(());
        };
        if self.v == LuaVersion::Lua51 {
            let arg: &[u8] = if name == "_G" { b"" } else { name.as_bytes() };
            self.pushcclosure(vm, f, 0)?;
            self.pushstring(vm, arg)?;
            return self.call(vm, 1, 0);
        }
        self.requiref(vm, name.as_bytes(), f, true)?;
        self.pop(vm, 1);
        Ok(())
    }
}
