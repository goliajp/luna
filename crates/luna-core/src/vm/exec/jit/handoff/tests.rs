//! The top a level compiled code handed over goes on with. Compiled code
//! sizes a constructor's table for its open call's value, so no allocation
//! fails at the `SETLIST` and no test program hands a level over there:
//! the frame is made here.

use crate::runtime::{Gc, LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::exec::Vm;
use crate::vm::isa::Op;

/// `f`'s constructor takes `g()`'s values up to the top; the locals after
/// it make the frame wider than the constructor's registers.
const F: &str = "local function g() return 1 end \
     local function f() local t = {g()} local a, b, c = 1, 2, 3 return t end return f";

fn function(vm: &mut Vm) -> Gc<LuaClosure> {
    let main = vm.load(F.as_bytes(), b"=t").unwrap();
    let r = vm.call_value(Value::Closure(main), &[]).unwrap();
    let Value::Closure(f) = r[0] else {
        panic!("no function returned")
    };
    f
}

/// The top after `f`'s level, handed over at `pc`, became a frame.
fn top_at(pc: usize) -> (u32, u32) {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let f = function(&mut vm);
    let max = u32::from(f.proto.max_stack);
    let func_slot = vm.top;
    let base = func_slot + 1;
    vm.jit.levels_slot = base;
    vm.grow_stack((base + max) as usize).unwrap();
    let vals = std::iter::repeat_n(Value::Nil, max as usize);
    vm.jit_handoff_frame(None, pc as u32, vals);
    vm.jit_resume_handoff(f, func_slot, 1, false).unwrap();
    (vm.top - base, max)
}

#[test]
fn a_level_handed_over_at_an_open_setlist_goes_on_with_its_call_s_value() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let code = function(&mut vm).proto.code.to_vec();
    let at = code
        .iter()
        .position(|i| i.op() == Op::SetList && i.b() == 0)
        .expect("an open SETLIST");
    let call = code[at - 1];
    assert!(call.op() == Op::Call && call.c() == 0);
    let (top, max) = top_at(at);
    assert!(call.a() + 1 < max, "the frame is no wider than the call");
    assert_eq!(top, call.a() + 1);
    // any other op goes on with the frame's top
    let (top, max) = top_at(at + 1);
    assert_eq!(top, max);
}
