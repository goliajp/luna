//! A host's protected call (lua.c's `docall`) counts no call level of its
//! own: when the function it calls returns, the levels in use are the
//! host's, never fewer. 5.1's `luaD_call` checks for a collection as each
//! call returns; one at the end of the host's call found a level less
//! than none (the native standing in for the call had left, still taken
//! off) and the collector shrank the thread with that count.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};

fn same_error(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let e = vm.nat_arg(fs, nargs, 0);
    Ok(vm.nat_return(fs, &[e]))
}

#[test]
fn a_host_call_returning_with_a_step_due_counts_no_level_below_none() {
    // C stack overflows of every kind, caught: the host's call returns
    // with a step due
    let src = include_str!("../diff_puc/5.1/870_c_stack_overflow_paths.lua");
    let mut vm = Vm::new(LuaVersion::Lua51);
    let cl = vm.load(src.as_bytes(), b"=levels").unwrap();
    let msgh = vm.native(same_error);
    let r = vm.call_value_with_handler_in_c(Value::Closure(cl), &[], msgh);
    assert!(r.is_ok(), "{:?}", r.err().map(|e| e.0));
}
