//! Running the script: the stack the calls work on, and the loop that
//! reads each call and makes it.
//!
//! The calls run either at the host level, where `luaL_openlibs` runs
//! (on the stack above everything else, as a host's C code pushes), or
//! inside a library's `luaopen_*`, a native that pushes on its own stack
//! window. Either way the values sit in the slots PUC's have, so a
//! collection marks them in PUC's order.

use super::{Script, native};
use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// An index of the C API, resolved: a stack slot, or a pseudo-index.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ix {
    Slot(u32),
    Registry,
    /// 5.1's `LUA_GLOBALSINDEX`
    Globals,
    /// 5.1's `LUA_ENVIRONINDEX`: the running function's environment
    Env,
}

/// The state of one run of the script's calls.
pub(crate) struct Opener {
    pub(super) s: &'static Script,
    pub(super) v: LuaVersion,
    /// the running `luaopen_*`'s slot; none at the host level
    pub(super) native: Option<u32>,
    /// the host level's stack top before the calls ran
    base: u32,
    /// 5.1: the environment of the running `luaopen_*` (none: the
    /// globals)
    pub(super) env: Option<Value>,
    /// luna's own objects the functions of this library share
    own: Vec<(&'static str, Value)>,
    /// a `luaL_register` without a library name runs: it fills the table
    /// on top (5.1's file methods)
    pub(super) methods: bool,
    /// `Vm::open_base` alone: the base library without `dofile` and
    /// `loadfile`
    pub(crate) sandbox: bool,
}

/// The operands of one call, read from the script.
pub(super) struct Rd<'a> {
    pub(super) b: &'a [u8],
    pub(super) i: usize,
}

impl Rd<'_> {
    /// An operand: a LEB128 varint.
    pub(super) fn u(&mut self) -> u64 {
        let mut n = 0u64;
        let mut shift = 0;
        loop {
            let b = self.b[self.i];
            self.i += 1;
            n |= u64::from(b & 0x7F) << shift;
            if b < 0x80 {
                return n;
            }
            shift += 7;
        }
    }

    /// A signed operand (zigzag).
    pub(super) fn z(&mut self) -> i64 {
        let n = self.u();
        (n >> 1) as i64 ^ -((n & 1) as i64)
    }
}

impl Opener {
    /// The calls at the host level of `vm`, above its stack's top.
    pub(crate) fn host(vm: &mut Vm, s: &'static Script) -> Opener {
        let base = vm.stack.len() as u32;
        Opener::new(vm.version(), s, None, base)
    }

    /// The calls of the native running at slot `fs`.
    pub(crate) fn native(vm: &Vm, s: &'static Script, fs: u32) -> Opener {
        Opener::new(vm.version(), s, Some(fs), 0)
    }

    fn new(v: LuaVersion, s: &'static Script, native: Option<u32>, base: u32) -> Opener {
        Opener {
            s,
            v,
            native,
            base,
            env: None,
            own: Vec::new(),
            methods: false,
            sandbox: false,
        }
    }

    /// The host level gives back what the calls left on its stack.
    pub(crate) fn leave(&mut self, vm: &mut Vm) {
        if self.native.is_none() {
            self.set_top(vm, self.base);
        }
    }

    /// The slot past the top value.
    pub(crate) fn top(&self, vm: &Vm) -> u32 {
        match self.native {
            None => vm.stack.len() as u32,
            Some(_) => vm.running_natives.last().map_or(0, |a| a.top()),
        }
    }

    /// Push `v` (C `api_incr_top`).
    pub(crate) fn push(&mut self, vm: &mut Vm, v: Value) -> Result<(), LuaError> {
        match self.native {
            None => {
                let n = vm.stack.len();
                vm.grow_stack_holding(n + 1, &[v])?;
                vm.stack[n] = v;
                vm.top = n as u32 + 1;
                Ok(())
            }
            Some(_) => vm.native_push_vals(&[v]),
        }
    }

    /// The top set to slot `t` (C `L->top = t`).
    pub(crate) fn set_top(&mut self, vm: &mut Vm, t: u32) {
        match self.native {
            None => {
                vm.stack.truncate(t as usize);
                vm.top = t;
            }
            Some(fs) => vm.native_settop(t - fs - 1),
        }
    }

    /// `n` values popped (C `lua_pop`).
    pub(crate) fn pop(&mut self, vm: &mut Vm, n: u32) {
        let t = self.top(vm);
        self.set_top(vm, t - n);
    }

    /// The index `idx` resolved.
    pub(crate) fn ix(&self, vm: &Vm, idx: i64) -> Ix {
        match (self.v, idx) {
            (LuaVersion::Lua51, -10002) => Ix::Globals,
            (LuaVersion::Lua51, -10001) => Ix::Env,
            (_, i) if i < -100_000 || (self.v == LuaVersion::Lua51 && i == -10000) => Ix::Registry,
            (_, i) if i < 0 => Ix::Slot((self.top(vm) as i64 + i) as u32),
            (_, i) => {
                let first = self.native.map_or(self.base, |fs| fs + 1);
                Ix::Slot(first + i as u32 - 1)
            }
        }
    }

    /// The value at `ix`.
    pub(crate) fn at(&self, vm: &Vm, ix: Ix) -> Value {
        match ix {
            Ix::Slot(s) => vm.stack[s as usize],
            Ix::Registry => vm.registry.map_or(Value::Nil, Value::Table),
            Ix::Globals => Value::Table(vm.globals()),
            Ix::Env => self.env.unwrap_or(Value::Table(vm.globals())),
        }
    }

    /// The value `idx` names.
    pub(crate) fn get(&self, vm: &Vm, idx: i64) -> Value {
        self.at(vm, self.ix(vm, idx))
    }

    /// 5.1's environment of the running `luaopen_*`.
    pub(crate) fn env_value(&self, vm: &Vm) -> Value {
        self.at(vm, Ix::Env)
    }

    /// The `n` values on top, bottom first.
    pub(crate) fn top_values(&self, vm: &Vm, n: u32) -> Vec<Value> {
        let t = self.top(vm) as usize;
        vm.stack[t - n as usize..t].to_vec()
    }

    /// PUC's upvalue `i` of a C closure of `nup` being made: the values on
    /// top, the first lowest.
    pub(crate) fn upval(&self, vm: &Vm, i: u32, nup: u32) -> Value {
        vm.stack[(self.top(vm) - nup + i) as usize]
    }

    /// A check for a collection with what the calls hold on the stack.
    fn check(&mut self, vm: &mut Vm) -> Result<(), LuaError> {
        match self.native {
            Some(_) => vm.native_checkpoint(),
            None if vm.checkpoint_idle() => Ok(()),
            None => {
                let t = vm.stack.len() as u32;
                vm.checkpoint(t)
            }
        }
    }

    /// The check an API call that allocates makes before it (5.1, 5.2).
    pub(crate) fn before(&mut self, vm: &mut Vm) -> Result<(), LuaError> {
        if self.v <= LuaVersion::Lua52 {
            return self.check(vm);
        }
        Ok(())
    }

    /// The check it makes after it pushed (5.3 on).
    pub(crate) fn after(&mut self, vm: &mut Vm) -> Result<(), LuaError> {
        if self.v >= LuaVersion::Lua53 {
            return self.check(vm);
        }
        Ok(())
    }

    /// The script's string `i` (1-based; 0 is none).
    pub(super) fn str(&self, i: u64) -> Option<&'static [u8]> {
        (i > 0).then(|| self.s.strs[i as usize - 1])
    }

    /// luna's own object `key` the functions being made share, made by
    /// `make` the first time.
    pub(crate) fn own(
        &mut self,
        vm: &mut Vm,
        key: &'static str,
        make: fn(&mut Vm) -> Result<Value, LuaError>,
    ) -> Result<Value, LuaError> {
        if let Some(&(_, v)) = self.own.iter().find(|(k, _)| *k == key) {
            return Ok(v);
        }
        let v = make(vm)?;
        self.own.push((key, v));
        Ok(v)
    }

    /// Run the calls `ops`.
    pub(crate) fn run(&mut self, vm: &mut Vm, ops: &[u8]) -> Result<(), LuaError> {
        let mut r = Rd { b: ops, i: 0 };
        while r.i < ops.len() {
            let op = ops[r.i];
            r.i += 1;
            self.step(vm, op, &mut r)?;
        }
        Ok(())
    }

    /// The function the script numbers `f`, as luna makes it, with PUC's
    /// `nup` upvalues on top (C `lua_pushcclosure`'s object).
    pub(super) fn make_fn(&mut self, vm: &mut Vm, f: u32, nup: u32) -> Result<Value, LuaError> {
        let (ctor, id) = native(f);
        match ctor {
            super::Ctor::F(g, b) => {
                let up: Vec<Value> = (0..nup).map(|i| self.upval(vm, i, nup)).collect();
                self.func(vm, id as u32, g, b, &up, nup)
            }
            super::Ctor::C(c) => c(vm, self, id as u32, nup),
            super::Ctor::Open(g) => {
                self.func(vm, id as u32, g, crate::runtime::Builtin::None, &[], nup)
            }
        }
    }
}
