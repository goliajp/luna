//! Loops whose registers change kind between iterations, run with every
//! threshold at its lowest: side traces are recorded at once, run from
//! their parent's code and end at its head, and the parent moves to the
//! optimizing tier at its first entry. A side trace that leaves a register
//! the parent writes before reading it of another kind than the parent's
//! loop is compiled for makes the parent leave at its head right after the
//! relink; the dispatcher must restore the registers with the kinds the
//! side trace left them in, or a collection then marks a number as a table.

use luna_core::__private::VmJitState;
use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

const CASES: [&str; 6] = [
    "local x, i = {}, 0 while i < 300 do i = i + 1 if i % 2 == 1 then x = {} else x = 7 end end collectgarbage() return type(x), x",
    "local x, i = {}, 0 while i < 300 do i = i + 1 if i % 2 == 1 then x = 7 else x = {} end end collectgarbage() return type(x)",
    "local x, i = 's', 0 while i < 300 do i = i + 1 if i % 2 == 1 then x = 7 else x = 's' .. i end end collectgarbage() return x",
    "local x, y = 0.5, 0 for i = 1, 300 do if i % 2 == 1 then y = y + x x = 7 else y = y + x x = 0.5 end end return y",
    "local t = {} for i = 1, 300 do t[i] = i end local x, y = 0.5, 0 for _, v in ipairs(t) do if v % 2 == 1 then y = y + x x = 7 else y = y + x x = 0.5 end end return x, y",
    "local x, y, i = 0.5, 0, 0 while i < 300 do i = i + 1 if i % 2 == 1 then y = y + x x = 7 else y = y + x x = 0.5 end end return y",
];

/// Each case of `CASES` (a global) loaded and run in its own function, its
/// results as text, one line per case.
const DRIVER: &str = r#"
    local ld = loadstring or load
    local out = {}
    for _, src in ipairs(CASES) do
      local r = {pcall(ld(src, "=c"))}
      for i = 1, #r do r[i] = tostring(r[i]) end
      out[#out + 1] = table.concat(r, " ")
    end
    return table.concat(out, "\n")"#;

fn run(v: LuaVersion, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    vm.set_trace_hot_threshold(1);
    vm.set_call_hot_threshold(1);
    vm.set_trace_tier_up_at(1);
    let cases: Vec<String> = CASES.iter().map(|c| format!("[[{c}]]")).collect();
    let src = format!("CASES = {{{}}}\n{DRIVER}", cases.join(", "));
    let out = match vm.eval(&src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    };
    (out, vm.jit_counters_mut().side_trace_compiled)
}

#[test]
fn a_return_at_the_head_right_after_a_relink_restores_the_side_traces_kinds() {
    for v in VERSIONS {
        let (interp, _) = run(v, false);
        let (jit, sides) = run(v, true);
        assert!(sides > 0, "{v:?}: no side trace compiled");
        assert_eq!(jit, interp, "{v:?}");
    }
}
