//! Trace shapes whose lowering once tripped the IR builder's own checks.
//! Those checks are debug assertions, so these run as unit tests: the
//! `--lib` run is a debug build.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;

fn run(src: &str, jit: bool) -> (String, u64) {
    let mut vm = super::test_vm_new(LuaVersion::Lua54);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let out = match vm.eval(src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("snippet must return a string, got {other:?}"),
    };
    (out, vm.trace_compiled_count())
}

/// A `math.floor` fold checked once before the loop head (nothing in the
/// trace can reassign it) used to read the loop-head registers before
/// the check's block was built.
#[test]
fn math_fold_checked_before_the_loop_head() {
    let src = r#"
        local last
        for i = 1, 2000 do local x = i last = math.floor(x) end
        return tostring(last)"#;
    let (interp, _) = run(src, false);
    let (jit, compiled) = run(src, true);
    assert_eq!(interp, "2000");
    assert_eq!(jit, interp);
    assert!(compiled > 0, "no trace was compiled");
}
