//! A `math.max` / `math.min` / `string.sub` call folded into a trace, left
//! between the library lookup and the call because an argument stops being
//! what the fold needs: the interpreter must run the call itself and raise
//! the library's argument error. The trace never loaded the function into
//! the call's register, so it has to leave at the lookup, not at the
//! argument.

use luna_jit::LuaVersion;
use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `ARR` is the array read, `LOOP` the loop around the folded call (it
/// binds `i`), `CALL` the call; the read at index 9 is nil.
const TEMPLATE: &str = r#"
    local arr = ARR
    local strs = {"ab", "cd", "ef", "gh", "ij", "kl", "mn", "op"}
    local t = {}
    local ok, err = pcall(function()
      LOOP
        arr[3] = CALL or 0
        t[i] = i + i
      end
    end)
    return tostring(ok) .. " " .. tostring(err) .. " " .. #t"#;

const CALLS: [&str; 3] = [
    "math.max(arr[i], 5)",
    "math.min(arr[i], 5)",
    "#string.sub(strs[i], 1, 1)",
];

const ARRAYS: [&str; 2] = [
    r#"{1, 2.5, "3", 4, 5, 6, 7, 8}"#,
    "{1, 2, 3, 4, 5, 6, 7, 8}",
];

const LOOPS: [&str; 3] = [
    "for i = 1, 17 do",
    "local i = 0 while i < 17 do i = i + 1",
    "for _, i in ipairs({1, 2, 3, 4, 5, 6, 7, 8, 9, 10}) do",
];

fn run(v: LuaVersion, src: &str, jit: Option<(TraceTier, u32, u32)>) -> String {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(jit.is_some());
    vm.set_trace_jit_enabled(jit.is_some());
    if let Some((tier, trace, call)) = jit {
        vm.jit.trace_tier = tier;
        vm.jit.trace_hot_threshold = trace;
        vm.jit.call_hot_threshold = call;
    }
    match vm.eval(src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn folded_math_call_left_with_a_non_number() {
    for v in DIALECTS {
        for call in CALLS {
            for arr in ARRAYS {
                for lp in LOOPS {
                    let src = TEMPLATE
                        .replace("ARR", arr)
                        .replace("LOOP", lp)
                        .replace("CALL", call);
                    let interp = run(v, &src, None);
                    assert!(interp.starts_with("false "), "{v:?}: {interp}\n{src}");
                    for tier in [TraceTier::Baseline, TraceTier::Optimizing, TraceTier::Auto] {
                        for (trace, call) in [(1, 7), (1, 1), (2, 2)] {
                            assert_eq!(
                                run(v, &src, Some((tier, trace, call))),
                                interp,
                                "{v:?} {tier:?}, trace hot {trace}, call hot {call}\n{src}"
                            );
                        }
                    }
                }
            }
        }
    }
}
