//! 5.1's library opening counted object by object. Each `luaopen_*`
//! registers new C closures into the library table it finds
//! (`luaI_openlib`) and makes a few objects of its own; luna opens the
//! library without counting, then counts PUC's objects in PUC's order,
//! each standing on the luna value it lives as long as. The objects the
//! opening replaces die with the luna values they stand on.

use super::*;
use crate::runtime::heap::{GcHeader, PucKind};
use crate::vm::exec::host_c::HostOpenError;

#[path = "reopen51/objects.rs"]
mod objects;

/// An object of PUC's, or a change to a block of one.
#[derive(Clone, Copy)]
enum Obj {
    /// a C closure with this many upvalues
    Closure(usize),
    /// a table of this many array slots and hash nodes
    Table(usize, usize),
    /// a full userdata of this many bytes
    Udata(usize),
    /// a block of an object above resized from one size to another
    Block(usize, usize),
}

impl Vm {
    /// Library `name` opened again in 5.1, counted as PUC counts it: the
    /// objects the library holds now are given the count PUC gave them
    /// (they go when the luna values they stand on go), the library opens
    /// without counting, and PUC's new objects are counted on the new luna
    /// values.
    pub(super) fn reopen_lib_51(
        &mut self,
        name: &str,
        open: Option<fn(&mut Vm) -> Result<(), LuaError>>,
    ) -> Result<(), LuaError> {
        let had = name == "_G" || self.lib_table_51(name)?.is_some();
        if had {
            let old = self.puc_objects_51(name, false)?;
            // the values that stand for nothing yet (made by the first,
            // recorded opening), before any of them gets an object
            let fresh: Vec<bool> = old
                .iter()
                .map(|(a, _)| self.stands_for_nothing(*a))
                .collect();
            for ((anchor, objs), fresh) in old.into_iter().zip(fresh) {
                if !fresh {
                    continue;
                }
                // `package`'s finalizer in `_LOADLIB` is replaced: garbage
                let anchor = match objs.as_slice() {
                    [Obj::Closure(0)] if name == "package" && matches!(anchor, Value::Table(_)) => {
                        Value::Nil
                    }
                    _ => anchor,
                };
                self.stand_51(anchor, &objs, false)?;
            }
        }
        // the C closure of `luaopen_*` the caller pushes and calls: garbage
        let c0 = self.heap.sizes.c_closure(0);
        self.heap.acct_obj(PucKind::CClosure, c0);
        self.retry_oom(|vm| vm.heap.ghost_record(PucKind::CClosure, c0))?;
        self.heap.puc.silent += 1;
        let r = self.open_lib_51(name, open);
        self.heap.puc.silent -= 1;
        r?;
        for (anchor, objs) in self.puc_objects_51(name, !had)? {
            self.stand_51(anchor, &objs, true)?;
        }
        Ok(())
    }

    /// The table `luaI_openlib` registers library `name` into: the one
    /// `package.loaded` or, failing that, the global holds.
    fn lib_table_51(&mut self, name: &str) -> Result<Option<Gc<Table>>, LuaError> {
        let loaded = self.host_loaded()?;
        let k = self.str_value(name.as_bytes())?;
        Ok(match (loaded.get(k), self.globals.get(k)) {
            (Value::Table(t), _) | (_, Value::Table(t)) => Some(t),
            _ => None,
        })
    }

    /// The library opened as its `luaopen_*` opens it, uncounted.
    fn open_lib_51(
        &mut self,
        name: &str,
        open: Option<fn(&mut Vm) -> Result<(), LuaError>>,
    ) -> Result<(), LuaError> {
        let opened = match open {
            Some(f) => self.open_lib_as(name, f),
            None => self.host_open_lib(name),
        };
        match opened {
            Ok(_) => Ok(()),
            Err(HostOpenError::Error(e)) => Err(e),
            Err(HostOpenError::Conflict(name)) => {
                let msg = format!("name conflict for module '{name}'");
                Err(LuaError(self.str_value(msg.as_bytes())?))
            }
        }
    }

    /// PUC's objects `objs` live as long as `anchor` (counted now with
    /// `count`; without, they were counted when they were made).
    fn stand_51(&mut self, anchor: Value, objs: &[Obj], count: bool) -> Result<(), LuaError> {
        let h = match anchor {
            // luna frees a table made without counting as counted, at the
            // size PUC's of the same shape has: nothing to stand on it
            Value::Table(_) => {
                if count {
                    self.count_51(objs);
                }
                return Ok(());
            }
            Value::Native(n) => n.as_ptr() as *mut GcHeader,
            Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
            _ => std::ptr::null_mut(),
        };
        let s = self.heap.sizes;
        let mut sizes = Vec::new();
        for &o in objs {
            match o {
                Obj::Closure(n) => {
                    let c = s.c_closure(n);
                    if count {
                        self.heap.acct_obj(PucKind::CClosure, c);
                    }
                    sizes.push(c);
                }
                Obj::Table(a, n) => {
                    let (ab, hb) = (self.heap.array_bytes_of(a), self.heap.hash_bytes_of(n));
                    if count {
                        self.heap.acct_obj(PucKind::Table, s.table);
                        self.heap.acct_block(0, ab);
                        self.heap.acct_block(0, hb);
                    }
                    sizes.extend([hb, ab, s.table].into_iter().filter(|&b| b > 0));
                }
                Obj::Udata(b) => {
                    let u = s.userdata(1, b);
                    if count {
                        self.heap.acct_obj(PucKind::Userdata, u);
                    }
                    sizes.push(u);
                }
                Obj::Block(from, to) => {
                    if count {
                        self.heap.acct_block(from, to);
                    }
                    if to > from {
                        sizes.push(to - from);
                    } else if let Some(i) = sizes.iter().rposition(|&x| x == from - to) {
                        sizes.remove(i);
                    }
                }
            }
        }
        if h.is_null() {
            // nothing of luna's holds it: garbage at once (a new object of
            // PUC's, or an old one the opening replaces)
            for b in sizes {
                self.retry_oom(|vm| vm.heap.ghost_record(PucKind::Table, b))?;
            }
            return Ok(());
        }
        self.heap.stand_sizes_on(h, sizes);
        Ok(())
    }

    /// Whether `v` is a value of luna's made without counting that no
    /// object of PUC's stands on yet.
    fn stands_for_nothing(&self, v: Value) -> bool {
        let h = match v {
            Value::Native(n) => n.as_ptr() as *mut GcHeader,
            Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
            Value::Table(_) => return true,
            _ => return true,
        };
        crate::runtime::heap::is_uncounted(h) && !self.heap.stands(h)
    }
}

impl Vm {
    /// PUC's objects `objs` counted, made now.
    fn count_51(&mut self, objs: &[Obj]) {
        let s = self.heap.sizes;
        for &o in objs {
            match o {
                Obj::Closure(n) => self.heap.acct_obj(PucKind::CClosure, s.c_closure(n)),
                Obj::Table(a, n) => {
                    self.heap.acct_obj(PucKind::Table, s.table);
                    let (ab, hb) = (self.heap.array_bytes_of(a), self.heap.hash_bytes_of(n));
                    self.heap.acct_block(0, ab);
                    self.heap.acct_block(0, hb);
                }
                Obj::Udata(b) => self.heap.acct_obj(PucKind::Userdata, s.userdata(1, b)),
                Obj::Block(from, to) => self.heap.acct_block(from, to),
            }
        }
    }
}
