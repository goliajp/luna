//! A message handler that fails at once runs again on its own error one C
//! level deeper each time, so how often it runs is set by the C levels, not
//! by how much native stack the thread has.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SRC: &str = r#"
local n = 0
debug.sethook(function () n = n + 1 end, "c")
local ok, msg = xpcall(error, error)
debug.sethook()
return tostring(ok) .. " " .. tostring(msg), n
"#;

/// `SRC` run in `v` on a thread with `stack` bytes: its message, and how
/// many calls the hook saw.
fn run(v: LuaVersion, stack: usize) -> (String, i64) {
    std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let mut vm = Vm::new(v);
            let out = vm.eval(SRC).expect("the chunk runs");
            let msg = match out[0] {
                Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => panic!("a string: {other:?}"),
            };
            let n = match out[1] {
                Value::Int(n) => n,
                // 5.1 and 5.2 count in floats
                Value::Float(n) => n as i64,
                other => panic!("a count: {other:?}"),
            };
            (msg, n)
        })
        .expect("spawn")
        .join()
        .expect("the thread finishes")
}

#[test]
fn handler_reruns_do_not_depend_on_the_native_stack() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let small = run(v, 1 << 20);
        let large = run(v, 64 << 20);
        assert_eq!(small.0, "false error in error handling", "{v:?}");
        assert_eq!(small, large, "{v:?}");
        assert!(small.1 < 250, "{v:?}: {} runs", small.1);
    }
}
