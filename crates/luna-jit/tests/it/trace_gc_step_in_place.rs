//! A collection step that falls due inside a trace runs there, with the
//! stack and the check the interpreter has after the op, and the trace
//! goes on: the counts the program reads are the interpreter's, and the
//! loop is not entered again for each step. A step that runs a finalizer
//! leaves the trace, which then must not write over what the finalizer
//! changed.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

// a loop that makes short strings and drops them, reading the count as
// it goes
const COUNTS: &str = r#"
local seen, last = {}, nil
for i = 1, 20000 do
  last = "item:" .. i .. ":" .. (i % 7)
  if i % 2000 == 0 then seen[#seen + 1] = string.format("%.0f", collectgarbage("count") * 1024) end
end
return table.concat(seen, ",") .. "|" .. last
"#;

// tables whose finalizers bump a counter the loop reads (5.2 on); each
// finalizer also clears the slot the loop keeps its last key in
const FINALIZERS: &str = r#"
local fin, out, last = 0, {}, nil
local mt = {__gc = function() fin = fin + 1 end}
for i = 1, 3000 do
  if i % 50 == 0 then setmetatable({}, mt) end
  last = "k" .. i .. "." .. fin
  if i % 300 == 0 then out[#out + 1] = last end
end
return table.concat(out, ",") .. "|" .. fin
"#;

fn run(v: LuaVersion, trace: bool, src: &str) -> (String, u64) {
    // one string hash seed for the runs compared
    let mut vm = super::trace_corpus::seeded(v, 1);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(trace);
    let out = match &vm.eval(src).unwrap()[0] {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    };
    (out, vm.trace_dispatched_count())
}

#[test]
fn a_step_in_a_trace_leaves_the_counts_the_interpreter_reads() {
    for v in VERSIONS {
        let (interp, _) = run(v, false, COUNTS);
        let (jit, dispatched) = run(v, true, COUNTS);
        assert_eq!(jit, interp, "{v:?}");
        // the first entry, one after each count read and a few per
        // collection cycle, not one per step (the loop makes hundreds)
        assert!(dispatched < 150, "{v:?}: {dispatched} entries");
    }
}

#[test]
fn a_finalizer_run_by_a_step_in_a_trace_is_seen_by_the_loop() {
    for v in &VERSIONS[1..] {
        let (interp, _) = run(*v, false, FINALIZERS);
        let (jit, _) = run(*v, true, FINALIZERS);
        assert_eq!(jit, interp, "{v:?}");
    }
}
