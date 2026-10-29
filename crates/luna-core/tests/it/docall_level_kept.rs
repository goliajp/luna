//! lua.c's `docall` from `pmain`: 5.1's call of the chunk checks for a
//! collection as it returns, with `pmain`'s closure still on the stack
//! below it. A step there must keep that closure: the count PUC reads
//! after the next chunk is the one it read after this.

use luna_core::__private::VmHostInternals;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Standalone, StandaloneOptions, Vm};

fn keep_message(_: &mut Vm, _: u32, _: u32) -> Result<u32, LuaError> {
    Ok(0)
}

fn docall(s: &mut Standalone, source: &str) -> Option<f64> {
    let vm = s.vm_mut();
    let f = vm
        .load_buffer(source.as_bytes(), b"=(command line)", None)
        .expect("a chunk");
    let msgh = vm.native(keep_message);
    let r = vm
        .call_value_with_handler_in_c(f, &[], msgh)
        .expect("the chunk runs");
    match r.first() {
        Some(&Value::Float(n)) => Some(n),
        Some(&Value::Int(n)) => Some(n as f64),
        _ => None,
    }
}

#[test]
fn a_step_as_a_chunk_returns_keeps_pmain() {
    let mut s = Vm::new_minimal(LuaVersion::Lua51)
        .into_standalone(&["lua"], None, StandaloneOptions::default())
        .expect("lua.c's state");
    // a whole cycle at every step, a step due at every check
    docall(
        &mut s,
        "collectgarbage('setpause', 0) collectgarbage('setstepmul', 1000000)",
    );
    let count = "return collectgarbage('count') * 1024";
    let a = docall(&mut s, count);
    let b = docall(&mut s, count);
    assert!(a.is_some());
    assert_eq!(a, b);
}
