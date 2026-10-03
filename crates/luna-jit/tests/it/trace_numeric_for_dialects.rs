//! Numeric `for` loops in traces, in every dialect: 5.1 / 5.2 step a float
//! index and compare it with the limit, 5.3 does the same in integers
//! (wrapping on overflow) or floats, 5.4 / 5.5 count integer loops down and
//! step float loops like 5.3. Each program runs under the interpreter, both
//! trace tiers, the move from one tier to the other, and the method JIT;
//! the results must agree, and where the loop runs long enough the counters
//! must show its trace was entered.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn interp(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

fn traced(v: LuaVersion, tier: TraceTier, tier_up_at: u32) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    // the method JIT would run whole functions instead of the interpreter
    // the trace recorder watches
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm.jit.call_hot_threshold = 8;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

fn method_jit(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_trace_jit_enabled(false);
    vm.jit.call_hot_threshold = 1;
    vm
}

/// `src` returns a function; what it returns on each of `calls` calls.
/// In 5.1 / 5.2 an integer the VM keeps stands for the double of the same
/// value, so numbers are shown as doubles there.
fn results(vm: &mut Vm, src: &str, calls: usize) -> Vec<String> {
    let dbl = vm.version() <= LuaVersion::Lua52;
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    (0..calls)
        .map(|_| match vm.call_value(Value::Closure(f), &[]) {
            Ok(v) => v
                .iter()
                .map(|v| show(v, dbl))
                .collect::<Vec<_>>()
                .join(", "),
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect()
}

fn show(v: &Value, dbl: bool) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        Value::Int(n) if dbl => format!("{:?}", *n as f64),
        v => format!("{v:?}"),
    }
}

const TIERS: [(&str, TraceTier, u32); 4] = [
    ("baseline", TraceTier::Baseline, 0),
    ("cranelift", TraceTier::Optimizing, 0),
    ("tier up at once", TraceTier::Auto, 1),
    ("tier up partway", TraceTier::Auto, 40),
];

/// Runs `src` in dialect `v` everywhere against the interpreter and
/// returns the interpreter's results and the trace entries of each tier.
fn agree(v: LuaVersion, src: &str) -> (Vec<String>, Vec<u64>) {
    let want = results(&mut interp(v), src, 4);
    let entered = TIERS
        .iter()
        .map(|&(name, tier, at)| {
            let mut vm = traced(v, tier, at);
            let got = results(&mut vm, src, 4);
            assert_eq!(got, want, "{v:?} {name}");
            vm.trace_dispatched_count()
        })
        .collect();
    let got = results(&mut method_jit(v), src, 4);
    assert_eq!(got, want, "{v:?} method JIT");
    (want, entered)
}

/// As [`agree`], and every trace tier entered a trace.
fn assert_traced(versions: &[LuaVersion], src: &str) -> Vec<String> {
    let mut first = Vec::new();
    for &v in versions {
        let (want, entered) = agree(v, src);
        for (k, n) in entered.into_iter().enumerate() {
            assert!(n > 0, "{v:?} {}: no trace ran", TIERS[k].0);
        }
        first.push(want[0].clone());
    }
    first
}

/// Collects the loop variable's values (a float as `%.17g`) and their
/// count, keeping the last six; the
/// loop body ends the loop after `cap` iterations.
fn collect(header: &str, cap: u32) -> String {
    format!(
        r#"
return function()
    local seen, n = {{}}, 0
    {header}
        n = n + 1
        seen[n] = i
        if n >= {cap} then break end
    end
    local out = {{}}
    for k = math.max(1, n - 5), n do
        local i = seen[k]
        local int = math.type and math.type(i) == "integer"
        out[#out + 1] = int and tostring(i) or string.format("%.17g", i)
    end
    return n .. ":" .. table.concat(out, ",")
end
"#
    )
}

#[test]
fn integer_steps_up_and_down() {
    let src = r#"
return function()
    local s, last = 0, 0
    for i = 1, 300 do s = s + i * 2 last = i end
    for i = 300, 1, -3 do s = s - i last = i end
    return s, last
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn float_steps_accumulate_rounding() {
    let src = collect("for i = 0, 30, 0.1 do", 1000);
    assert_traced(&ALL, &src);
}

#[test]
fn negative_float_step() {
    assert_traced(&ALL, &collect("for i = 10, -10, -0.25 do", 1000));
}

#[test]
fn step_from_a_variable() {
    let src = r#"
return function()
    local s = 0
    for _, st in ipairs({3, -3, 7}) do
        for i = (st > 0 and 1 or 300), (st > 0 and 300 or 1), st do s = s + i end
    end
    return s
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn loop_variable_changed_in_the_body() {
    let src = r#"
return function()
    local s = 0
    for i = 1, 200 do i = i * 2 s = s + i end
    return s
end
"#;
    // the loop variable is constant in 5.5
    assert_traced(&ALL[..4], src);
}

#[test]
fn nested_loops_exit_and_reenter() {
    let src = r#"
return function()
    local s = 0
    for i = 1, 40 do
        for j = i, 1, -1 do s = s + j end
    end
    return s
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn loop_variable_as_array_key() {
    let src = r#"
return function()
    local t, s = {}, 0
    for i = 1, 200 do t[i] = i * 3 end
    for i = 1, 200 do s = s + t[i] end
    for i = 200, 1, -2 do s = s - t[i] end
    return s, #t
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn fractional_float_keys() {
    let src = r#"
return function()
    local t, s, n = {}, 0, 0
    for i = 0.5, 100, 0.5 do t[i] = i end
    for i = 0.5, 100, 0.5 do s = s + t[i] end
    for k in pairs(t) do n = n + 1 end
    return s, n
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn integer_state_kept_by_the_vm_in_51_52() {
    // `#t` is an integer the VM keeps; 5.1 / 5.2 still step the loop
    // as doubles
    let src = r#"
return function()
    local t, s = {}, 0
    for i = 1, 150 do t[i] = i end
    for i = #t, 1, -1 do s = s + i end
    for i = #t - 100, #t do s = s + t[i] end
    return s
end
"#;
    assert_traced(&ALL, src);
}

#[test]
fn doubles_round_past_two_to_the_53() {
    // 2^53 + 1 rounds back to 2^53: the float loop stops moving
    let r = assert_traced(&ALL, &collect("for i = 2^53 - 20, 2^53 + 4 do", 60));
    assert!(r[0].starts_with("60:"), "{}", r[0]);
}

#[test]
fn infinite_limits() {
    assert_traced(&ALL, &collect("for i = 1, 1/0 do", 50));
    assert_traced(&ALL, &collect("for i = 1, -1/0, -1 do", 50));
    assert_traced(&ALL, &collect("for i = 0.5, 1/0, 0.5 do", 50));
}

#[test]
fn nan_limits() {
    // no iteration in 5.1 / 5.2 (every comparison with NaN fails); in 5.3
    // an integer loop clamps the limit and, with a negative step, runs on
    let src = collect("for i = 1, 0/0, -1 do", 50);
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(agree(v, &src).0[0], "0:", "{v:?}");
    }
    assert_traced(
        &[LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55],
        &src,
    );
    let src = collect("for i = 1, 0/0 do", 50);
    for v in ALL {
        let (r, _) = agree(v, &src);
        assert_eq!(r[0], "0:", "{v:?}");
    }
}

#[test]
fn zero_step_before_54() {
    let pre = [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53];
    // with a zero step the comparison is limit <= index
    assert_traced(&pre, &collect("for i = 10, 1, 0 do", 50));
    assert_traced(&pre, &collect("for i = 10.5, 1, 0 do", 50));
    let src = collect("for i = 1, 10, 0 do", 50);
    for v in pre {
        assert_eq!(agree(v, &src).0[0], "0:", "{v:?}");
    }
    // 5.3 floors a float limit for an integer loop, so this one never
    // ends there; 5.1 / 5.2 compare 1.5 <= 1 and never start
    let src = collect("for i = 1, 1.5, 0 do", 50);
    let r = assert_traced(&[LuaVersion::Lua53], &src);
    assert!(r[0].starts_with("50:"), "{}", r[0]);
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(agree(v, &src).0[0], "0:", "{v:?}");
    }
    // a NaN limit with a zero step: 5.3 restarts the index at 0
    let src = collect("for i = 1, 0/0, 0 do", 50);
    let r = assert_traced(&[LuaVersion::Lua53], &src);
    assert!(r[0].starts_with("50:0,"), "{}", r[0]);
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let r = agree(v, &src).0;
        assert!(r[0].contains("'for' step is zero"), "{v:?} {}", r[0]);
    }
}

#[test]
fn integer_overflow_wraps_in_53() {
    let up = collect("for i = math.maxinteger - 20, math.maxinteger do", 40);
    let r = assert_traced(&[LuaVersion::Lua53], &up);
    // past maxinteger the 5.3 index wraps to mininteger and goes on
    assert!(r[0].starts_with("40:-922337203685477"), "{}", r[0]);
    let r = assert_traced(&[LuaVersion::Lua54, LuaVersion::Lua55], &up);
    assert!(r[0].starts_with("21:"), "{}", r[0]);
    let down = collect("for i = math.mininteger + 20, math.mininteger, -1 do", 40);
    assert_traced(
        &[LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55],
        &down,
    );
}

#[test]
fn numeric_string_bounds() {
    // a string bound makes a float loop from 5.3
    assert_traced(&ALL, &collect(r#"for i = "1", "40" do"#, 100));
}

#[test]
fn short_branch_stays_in_the_trace() {
    // the branch not taken while recording is taken in the trace, so the
    // loop is entered once per call
    let src = r#"
return function()
    local last, s = 0, 0
    for i = 1, 3000 do
        if i % 3 == 0 then last = i end
        s = s + last
    end
    return s, last
end
"#;
    for v in ALL {
        let (_, entered) = agree(v, src);
        for (k, n) in entered.into_iter().enumerate() {
            assert!(n > 0 && n < 20, "{v:?} {}: entered {n} times", TIERS[k].0);
        }
    }
}

#[test]
fn instruction_budget_keeps_loops_out_of_traces() {
    // compiled code does not tick the budget, so no trace may run while
    // one is armed, even after the loop was compiled without one
    for v in ALL {
        let mut vm = traced(v, TraceTier::Auto, 0);
        vm.eval("local s = 0 for i = 1, 5000 do s = s + i end")
            .expect("warm");
        assert!(
            vm.trace_dispatched_count() > 0,
            "{v:?}: no trace before the budget"
        );
        let before = vm.trace_dispatched_count();
        vm.set_instr_budget(Some(50_000));
        let err = vm.eval("for i = 1, 1000000000 do end").unwrap_err();
        let msg = vm.error_text(&err);
        assert!(msg.contains("instruction budget exceeded"), "{v:?}: {msg}");
        assert_eq!(vm.trace_dispatched_count(), before, "{v:?}");
    }
}
