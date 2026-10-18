//! The coroutine library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::{Builtin, Value};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use crate::vm::opening::{Ctor, Natives, Opener, run_open};

use Ctor::{C, F};

pub(crate) const PUC_FNS: Natives = &[
    ("luaB_close", F(co_close, Builtin::None)),
    ("luaB_cocreate", F(co_create, Builtin::None)),
    ("luaB_coresume", C(co_resume_fn)),
    ("luaB_corunning", F(co_running, Builtin::None)),
    ("luaB_costatus", F(co_status, Builtin::None)),
    ("luaB_cowrap", C(co_wrap_fn)),
    ("luaB_yield", F(co_yield, Builtin::None)),
    ("luaB_yieldable", F(co_isyieldable, Builtin::None)),
    ("luaopen_coroutine", Ctor::Open(luaopen_coroutine)),
];

fn luaopen_coroutine(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_coroutine", 1)
}

/// The threads now inside a wrapped call, which `resume` and `wrap` share:
/// luna's own table, which PUC has none of.
fn in_wrap(vm: &mut Vm) -> Result<Value, LuaError> {
    vm.heap.puc.silent += 1;
    let w = vm.alloc_table();
    vm.heap.puc.silent -= 1;
    let w = w?;
    vm.heap.own_table(w);
    Ok(Value::Table(w))
}

fn co_resume_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let w = op.own(vm, "in_wrap", in_wrap)?;
    op.func(vm, id, co_resume, Builtin::None, &[w], nup)
}

fn co_wrap_fn(vm: &mut Vm, op: &mut Opener, id: u32, nup: u32) -> Result<Value, LuaError> {
    let w = op.own(vm, "in_wrap", in_wrap)?;
    op.func(vm, id, co_wrap, Builtin::None, &[w], nup)
}
