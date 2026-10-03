//! The context of one C API call: the Vm, the thread `L` names, its stack
//! as C sees it, and raising an error for the C side to throw.

use super::*;
use luna_core::runtime::{NativeClosure, Table};

/// Native upvalue 0 of a C function holds its pointer, 1 its 5.1
/// environment; the C upvalues follow.
pub(super) const C_UPVALS: usize = 2;

/// Where an index points (PUC `index2value`).
#[derive(Clone, Copy)]
pub(super) enum Slot {
    /// a stack slot, as an index into the thread's C stack
    Stack(usize),
    Registry,
    /// 5.1 `LUA_GLOBALSINDEX`: the thread's globals
    Globals,
    /// 5.1 `LUA_ENVIRONINDEX`: the running C function's environment
    Environ,
    /// the running C function's upvalue, from 1
    Upvalue(usize),
    /// an acceptable index with no value there
    Absent,
}

/// PUC `LUA_REGISTRYINDEX` of each dialect.
pub(super) fn registry_index(v: LuaVersion) -> c_int {
    match v {
        LuaVersion::Lua51 => -10000,
        LuaVersion::Lua55 => -(c_int::MAX / 2 + 1000),
        _ => -1_000_000 - 1000,
    }
}

/// One API call on the thread `l`.
pub(super) struct Api<'a> {
    pub(super) vm: &'a mut Vm,
    pub(super) l: *mut LuaState,
}

impl<'a> Api<'a> {
    /// The context of an API call on `l`.
    ///
    /// # Safety
    /// `l` is a live thread of an open state, and this call is the
    /// innermost API call running on the state; the result is dropped
    /// before the call returns.
    pub(super) unsafe fn new(l: *mut LuaState) -> Api<'a> {
        // SAFETY: the caller's contract: `l`'s global record is live and its
        // `vm` field is the pointer the innermost Rust caller of C handed
        // down, which nothing else uses while this call runs
        unsafe {
            Api {
                vm: &mut *(*(*l).g).vm,
                l,
            }
        }
    }

    /// The state record of `l`; short-lived, as a nested call into C may
    /// change it.
    pub(super) fn st(&mut self) -> &mut LuaState {
        // SAFETY: `l` is live (`Api::new`), and the borrow ends before any
        // call that could reach C and touch the record again
        unsafe { &mut *self.l }
    }

    pub(super) fn g(&mut self) -> &mut Global {
        // SAFETY: the global record outlives every thread (`Api::new`); the
        // borrow is as short as `st`'s
        unsafe { &mut *(*self.l).g }
    }

    pub(super) fn thread(&self) -> Gc<Coro> {
        // SAFETY: `l` is live (`Api::new`); one field is read
        unsafe { (*self.l).thread }
    }

    pub(super) fn version(&self) -> LuaVersion {
        self.vm.version()
    }

    pub(super) fn vnum(&self) -> c_int {
        version_num(self.vm.version())
    }

    /// The thread's C stack.
    pub(super) fn stack(&self) -> &Vec<Value> {
        // SAFETY: the thread is live while its state is, and the C stack is
        // written only through `stack_mut`, never while this borrow lives
        unsafe { &(*self.thread().as_ptr()).host_stack }
    }

    pub(super) fn stack_mut(&mut self) -> &mut Vec<Value> {
        // SAFETY: as `stack`; the exclusive borrow of `self` keeps any other
        // access to the C stack through this call out
        unsafe { &mut (*self.thread().as_ptr()).host_stack }
    }

    pub(super) fn base(&mut self) -> usize {
        self.st().base
    }

    pub(super) fn top(&self) -> usize {
        self.stack().len()
    }

    /// PUC `lua_gettop`.
    pub(super) fn gettop(&mut self) -> c_int {
        (self.top() - self.base()) as c_int
    }

    pub(super) fn push(&mut self, v: Value) {
        self.stack_mut().push(v);
        let co = self.thread();
        self.vm.heap.barrier_back(co);
    }

    pub(super) fn push_all(&mut self, vs: &[Value]) {
        self.stack_mut().extend_from_slice(vs);
        let co = self.thread();
        self.vm.heap.barrier_back(co);
    }

    /// Pop the top value; nil on an empty frame.
    pub(super) fn pop(&mut self) -> Value {
        if self.top() > self.base() {
            self.stack_mut().pop().unwrap_or(Value::Nil)
        } else {
            Value::Nil
        }
    }

    /// Pop the top `n` values, bottom first.
    pub(super) fn pop_n(&mut self, n: usize) -> Vec<Value> {
        let from = self.top().saturating_sub(n).max(self.base());
        self.stack_mut().split_off(from)
    }

    /// Drop the stack down to C stack index `to`.
    pub(super) fn truncate(&mut self, to: usize) {
        self.stack_mut().truncate(to);
    }

    /// Where `idx` points.
    pub(super) fn slot(&mut self, idx: c_int) -> Slot {
        let reg = registry_index(self.version());
        let base = self.base();
        let top = self.top();
        if idx > 0 {
            let i = base + idx as usize - 1;
            return if i < top {
                Slot::Stack(i)
            } else {
                Slot::Absent
            };
        }
        if idx < 0 && idx > reg {
            let d = (-idx) as usize;
            return if d <= top - base {
                Slot::Stack(top - d)
            } else {
                Slot::Absent
            };
        }
        if idx == reg {
            return Slot::Registry;
        }
        if idx == 0 {
            return Slot::Absent;
        }
        if self.version() == LuaVersion::Lua51 {
            match idx {
                -10001 => Slot::Environ,
                -10002 => Slot::Globals,
                _ => Slot::Upvalue((-10002 - idx) as usize),
            }
        } else {
            Slot::Upvalue((reg - idx) as usize)
        }
    }

    /// The C function running on this thread, if any.
    pub(super) fn running_c(&mut self) -> Option<Gc<NativeClosure>> {
        self.st().calls.last().map(|c| c.nc)
    }

    /// The thread's globals (5.1 `LUA_GLOBALSINDEX`).
    pub(super) fn thread_globals(&self) -> Gc<Table> {
        self.vm.host_thread_globals(self.thread())
    }

    /// The value at `idx`; `None` where PUC finds no value (`LUA_TNONE`).
    pub(super) fn get(&mut self, idx: c_int) -> Option<Value> {
        match self.slot(idx) {
            Slot::Stack(i) => Some(self.stack()[i]),
            Slot::Registry => Some(Value::Table(self.vm.host_registry())),
            Slot::Globals => Some(Value::Table(self.thread_globals())),
            Slot::Environ => {
                let env = self.running_c().map(|nc| nc.upvals[1]);
                Some(match env {
                    Some(Value::Table(t)) => Value::Table(t),
                    _ => Value::Table(self.thread_globals()),
                })
            }
            Slot::Upvalue(n) => {
                let nc = self.running_c()?;
                (n >= 1).then(|| nc.upvals.get(C_UPVALS + n - 1).copied())?
            }
            Slot::Absent => None,
        }
    }

    /// The value at `idx`, nil where there is none.
    pub(super) fn get_or_nil(&mut self, idx: c_int) -> Value {
        self.get(idx).unwrap_or(Value::Nil)
    }

    /// Store `v` where `idx` points (`lua_copy`, `lua_replace`).
    pub(super) fn set(&mut self, idx: c_int, v: Value) {
        match self.slot(idx) {
            Slot::Stack(i) => {
                self.stack_mut()[i] = v;
                let co = self.thread();
                self.vm.heap.barrier_back(co);
            }
            Slot::Upvalue(n) if n >= 1 => {
                if let Some(nc) = self.running_c()
                    && C_UPVALS + n - 1 < nc.upvals.len()
                {
                    // SAFETY: the running C function's closure is rooted by its
                    // call's stack slot; no other reference into it is live, and
                    // the borrow covers one store
                    unsafe { nc.as_mut() }.upvals[C_UPVALS + n - 1] = v;
                    self.vm.heap.barrier_back(nc);
                }
            }
            Slot::Environ => {
                if let (Some(nc), Value::Table(_)) = (self.running_c(), v) {
                    // SAFETY: as for an upvalue above
                    unsafe { nc.as_mut() }.upvals[1] = v;
                    self.vm.heap.barrier_back(nc);
                }
            }
            Slot::Globals => {
                if let Value::Table(t) = v {
                    let co = self.thread();
                    self.vm.host_set_thread_globals(co, t);
                }
            }
            // the registry and absent slots cannot be replaced
            Slot::Registry | Slot::Absent | Slot::Upvalue(_) => {}
        }
    }

    /// The C stack index of a stack index `idx` (positive or negative).
    pub(super) fn abs_stack(&mut self, idx: c_int) -> Option<usize> {
        match self.slot(idx) {
            Slot::Stack(i) => Some(i),
            _ => None,
        }
    }

    /// Raise `e` from this call: its object goes on top of the stack and
    /// the C side throws `status` once the Rust side has returned.
    pub(super) fn raise_status(&mut self, e: LuaError, status: c_int) {
        self.push(e.0);
        let l = self.l;
        let g = self.g();
        g.err_from = l;
        g.raised = status;
    }

    pub(super) fn raise(&mut self, e: LuaError) {
        self.raise_status(e, LUA_ERRRUN);
    }

    /// Raise a runtime error with message `msg`, with no position (PUC
    /// `luaG_runerror` from inside a C function).
    pub(super) fn raise_msg(&mut self, msg: &str) {
        let s = Value::Str(self.vm.heap.intern(msg.as_bytes()));
        self.raise(LuaError(s));
    }

    /// Whether this call raised.
    pub(super) fn raised(&mut self) -> bool {
        self.g().raised != 0
    }

    /// Intern `bytes` as a string value.
    pub(super) fn str(&mut self, bytes: &[u8]) -> Value {
        Value::Str(self.vm.heap.intern(bytes))
    }
}
