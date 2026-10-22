//! 5.1's environments in the opening: the package library makes its
//! table the environment of its loaders, `module` and `require`; the io
//! library makes three tables of its own (`newfenv`) the environments of
//! its functions and files, each holding a closure. luna keeps no objects
//! for those tables and closures but records (`heap_env51.rs`); here they
//! are counted where PUC makes them, and on the stack they are light
//! userdata naming their record, which a collection marks as PUC marks
//! the tables there.

use super::run::Opener;
use crate::runtime::Value;
use crate::runtime::heap::{ENV_IO, ENV_IO_POPEN, ENV_IO_STD, ENV_PACKAGE, PucKind};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

impl Opener {
    /// The io library's `newfenv` table `k` (0 to 2, as `heap_env51.rs`
    /// numbers them), made and pushed (`lua_createtable(L, 0, 1)`).
    pub(crate) fn newfenv(&mut self, vm: &mut Vm, k: usize) -> Result<(), LuaError> {
        self.before(vm)?;
        let s = vm.heap.puc.sizes;
        vm.heap.acct_obj(PucKind::Table, s.table);
        vm.heap.acct_block(0, s.node);
        let h = vm.heap.env51_born(k);
        vm.heap.puc.phantoms = true;
        self.push(vm, Value::LightUserdata(h as *const ()))?;
        self.after(vm)
    }

    /// The closure `k` (0 to 2: `io_fclose`, `io_noclose`, `io_pclose`)
    /// one of those tables holds, made.
    pub(crate) fn phantom_closure(&mut self, vm: &mut Vm, k: usize) -> Value {
        let s = vm.heap.puc.sizes;
        vm.heap.acct_obj(PucKind::CClosure, s.c_closure(0));
        Value::LightUserdata(vm.heap.env51_born(3 + k) as *const ())
    }

    /// The class bits of the environment `env` (none: the globals).
    fn bits_of(&self, vm: &Vm, env: Option<Value>) -> u8 {
        match env {
            Some(Value::LightUserdata(p)) => match vm.heap.env51_record_index(p as *mut _) {
                Some(0) => ENV_IO,
                Some(1) => ENV_IO_STD,
                _ => ENV_IO_POPEN,
            },
            Some(Value::Table(t)) if !t.ptr_eq(vm.globals()) => ENV_PACKAGE,
            _ => 0,
        }
    }

    /// The class bits of the running `luaopen_*`'s environment, which a C
    /// closure or userdata it makes gets (`getcurrenv`).
    pub(crate) fn env_bits(&self, vm: &Vm) -> u8 {
        self.bits_of(vm, self.env)
    }

    /// `lua_replace(L, LUA_ENVIRONINDEX)`: `env` the running function's
    /// environment.
    pub(crate) fn set_env(&mut self, vm: &mut Vm, env: Value) {
        self.env = Some(env);
        if let Value::Table(t) = env
            && !t.ptr_eq(vm.globals())
        {
            vm.heap.env51_package(t);
        }
        let bits = self.env_bits(vm);
        if self.native.is_some()
            && let Some(a) = vm.running_natives.last()
        {
            let h = a.nc.as_ptr() as *mut _;
            vm.heap.set_env51(h, bits);
            env_barrier(vm, h, env);
        }
    }

    /// `lua_setfenv(L, idx)`: the environment on top given to the object
    /// at `ix`, popped.
    pub(crate) fn setfenv(&mut self, vm: &mut Vm, ix: super::run::Ix) -> Result<(), LuaError> {
        let top = self.top(vm) as usize;
        let bits = self.bits_of(vm, Some(vm.stack[top - 1]));
        let h: *mut crate::runtime::heap::GcHeader = match self.at(vm, ix) {
            Value::Native(n) => n.as_ptr() as *mut _,
            Value::Userdata(u) => u.as_ptr() as *mut _,
            _ => std::ptr::null_mut(),
        };
        if !h.is_null() {
            vm.heap.set_env51(h, bits);
            env_barrier(vm, h, vm.stack[top - 1]);
        }
        self.pop(vm, 1);
        Ok(())
    }

    /// `lua_rawseti` into a record's table: its array part grows to `n`
    /// (a resize that keeps its one node), and the io library's first
    /// table holds the default files.
    pub(crate) fn phantom_seti(&mut self, vm: &mut Vm, t: Value, n: i64, v: Value) {
        let Value::LightUserdata(p) = t else {
            return;
        };
        if vm.heap.env51_record_index(p as *mut _) != Some(0) {
            return;
        }
        let n = n as usize;
        vm.heap.acct_table_resize((n - 1, 1), (n, 1));
        // `luaC_barriert`: the table, black, takes a new (white) file
        // SAFETY: a record lives as long as the heap
        unsafe { vm.heap.barrier_back_obj(p as *mut _) };
        if let Value::Userdata(u) = v {
            vm.heap.env51_set_io(n - 1, u);
        }
    }
}

/// `luaC_objbarrier` of 5.1's `lua_setfenv` and `lua_replace`: the object
/// behind `h`, black, takes the environment `env` (a record or a table).
fn env_barrier(vm: &mut Vm, h: *mut crate::runtime::heap::GcHeader, env: Value) {
    let e: *mut crate::runtime::heap::GcHeader = match env {
        Value::LightUserdata(p) => p as *mut _,
        Value::Table(t) => t.as_ptr() as *mut _,
        _ => return,
    };
    // SAFETY: `h` heads a live object and `e` a record or a live table
    unsafe { vm.heap.barrier_forward_obj(h, e) };
}
