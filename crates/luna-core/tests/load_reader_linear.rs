//! Loading a binary chunk through a reader that hands it over one byte at
//! a time, and looks at what `load` holds below it at every call, takes
//! time in proportion to the chunk: the undumper's walk picks up where it
//! stopped, and what the debug library was shown is brought up to date
//! rather than made again.
//!
//! A test binary of its own: the time is the process's CPU time
//! (`os.clock`), which no test on another thread may add to and which,
//! unlike the time on the clock, other processes do not add to either.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = r#"
local n = ...
local parts = {}
for i = 1, n do parts[#parts + 1] = string.format("'s%d', %d.5", i, i) end
local bin = string.dump((loadstring or load)("local t = {" .. table.concat(parts, ", ") .. "} return function() return t end"))
local k = 0
local start = os.clock()
local f = load(function()
  k = k + 1
  for i = 1, 8 do debug.getlocal(2, i) end
  return bin:sub(k, k)
end)
assert(f, "loaded")
return os.clock() - start
"#;

/// The shortest of a few loads of the chunk with `n` constants, in CPU
/// seconds.
fn seconds(v: LuaVersion, n: i64) -> f64 {
    (0..5)
        .map(|_| {
            let mut vm = Vm::new(v);
            let f = vm.load(SCRIPT.as_bytes(), b"=linear").expect("script");
            let r = vm
                .call_value(Value::Closure(f), &[Value::Int(n)])
                .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_display(&e)));
            match r.first() {
                Some(&Value::Float(t)) => t,
                other => panic!("{v:?}: a time, got {other:?}"),
            }
        })
        .fold(f64::INFINITY, f64::min)
}

#[test]
fn a_binary_chunk_read_a_byte_at_a_time_takes_linear_time() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let (small, large) = (seconds(v, 1500), seconds(v, 6000));
        // four times the chunk: about four times the time, where a walk
        // made again at each call would take sixteen
        assert!(large < small * 8.0, "{v:?}: {small:.3}s then {large:.3}s");
    }
}
