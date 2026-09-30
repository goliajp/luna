//! A looping trace is lowered once, for the register kinds it was
//! entered with, and its exits tag each register with the kind the
//! lowering assumed there. Two ways broke that and let a register's
//! bits be read as another kind:
//!
//! * on 5.1 / 5.2 the `math.min` / `math.max` fold kept an integer
//!   argument as an integer, where those dialects return a float, so
//!   the trace's idea of the result's kind was not the interpreter's
//!   (`math.min(#t, 2^53)` printed `4.4465908125712e-321`, the bits of
//!   the integer 900 read as a float);
//! * a back-edge ran the body again even when an iteration ended with
//!   a register holding another kind than on entry (a variable that is
//!   an integer on one branch and a float on the other), so the next
//!   iteration's exits restored that register with the entry kind.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: Option<(bool, u32)>) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    match jit {
        Some((method, hot)) => {
            vm.set_jit_enabled(method);
            vm.set_trace_jit_enabled(true);
            vm.jit.trace_hot_threshold = hot;
            vm.jit.call_hot_threshold = hot;
        }
        None => {
            vm.set_jit_enabled(false);
            vm.set_trace_jit_enabled(false);
        }
    }
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

/// Runs `src` with the trace JIT at several thresholds (with and
/// without the method JIT) and compares with the interpreter, whose
/// output must be `want[version]`. Some trace has to have run.
fn assert_same(src: &str, want: &[(LuaVersion, &str)]) {
    let mut bad = Vec::new();
    let mut dispatched = 0;
    for &(v, expect) in want {
        let (interp, _) = run(v, src, None);
        assert_eq!(interp, expect, "{v:?}: interpreter");
        for method in [false, true] {
            for hot in [1, 2, 3, 7] {
                let (jit, d) = run(v, src, Some((method, hot)));
                dispatched += d;
                if jit != interp {
                    bad.push(format!("{v:?} method {method} hot {hot}: {jit}"));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "JIT differs from the interpreter:\n{}",
        bad.join("\n")
    );
    assert!(dispatched > 0, "no trace ran");
}

fn all(want: &str) -> Vec<(LuaVersion, &str)> {
    [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ]
    .into_iter()
    .map(|v| (v, want))
    .collect()
}

/// The loop the fuzzer found, with the call in its place.
fn minmax_loop(call: &str) -> String {
    format!(
        r#"
        local f1, f2 = 3.5, -4.25
        local seq = {{}}
        for i = 1, 900 do seq[i] = i end
        local n2
        local w9 = 0
        while w9 < 300 do
          w9 = w9 + 1
          n2 = ((0.5 % 3) / w9)
          if f1 == f1 then n2 = {call} end
        end
        return tostring(n2)"#
    )
}

#[test]
fn min_of_an_integer_and_a_float() {
    assert_same(&minmax_loop("math.min(#seq, (2^53 + (- f2)))"), &all("900"));
}

#[test]
fn max_of_an_integer_and_a_float() {
    assert_same(&minmax_loop("math.max(#seq, -(2^53 + (- f2)))"), &all("900"));
}

#[test]
fn min_of_two_integers() {
    assert_same(&minmax_loop("math.min(#seq, 1000)"), &all("900"));
    assert_same(&minmax_loop("math.max(#seq, 10)"), &all("900"));
}

#[test]
fn min_of_a_float_and_an_integer() {
    assert_same(&minmax_loop("math.min(2^53 + (- f2), #seq)"), &all("900"));
}

/// 5.1 / 5.2 `math.min(0, 1)` is the float 0, whose negation is -0,
/// so `1 / -math.min(#t, 5)` is -inf there and inf from 5.3.
#[test]
fn minmax_result_is_a_float_before_5_3() {
    let neg = |call: &str| {
        format!(
            r#"
            local t = {{}}
            local r
            local i = 0
            while i < 300 do
              i = i + 1
              local m = {call}
              r = -m
            end
            return tostring(1 / r)"#
        )
    };
    let want = [
        (LuaVersion::Lua51, "-inf"),
        (LuaVersion::Lua52, "-inf"),
        (LuaVersion::Lua53, "inf"),
        (LuaVersion::Lua54, "inf"),
        (LuaVersion::Lua55, "inf"),
    ];
    assert_same(&neg("math.min(#t, 5)"), &want);
    assert_same(&neg("math.max(#t, -5)"), &want);
}

/// `x` is an integer after odd iterations and a float after even ones,
/// or the other way round; the loop's last exit must hand back the
/// value the last iteration stored.
#[test]
fn variable_changes_kind_across_a_while_back_edge() {
    let src = |odd: &str, even: &str| {
        format!(
            r#"
            local x = 0.5
            local i = 0
            while i < 300 do
              i = i + 1
              if i % 2 == 1 then x = {odd} else x = {even} end
            end
            return tostring(x)"#
        )
    };
    assert_same(&src("7", "0.5"), &all("0.5"));
    assert_same(&src("0.5", "7"), &all("7"));
}

/// Both branches read `x` before storing their own kind in it, so an
/// exit from the next iteration that restores `x` with the wrong kind
/// is seen by the interpreter.
#[test]
fn variable_changes_kind_across_a_numeric_for_back_edge() {
    let src = r#"
        local x, y = 0.5, 0
        for i = 1, 300 do
          if i % 2 == 1 then y = y + x x = 7 else y = y + x x = 0.5 end
        end
        return tostring(y)"#;
    assert_same(
        src,
        &[
            (LuaVersion::Lua51, "1125"),
            (LuaVersion::Lua52, "1125"),
            (LuaVersion::Lua53, "1125.0"),
            (LuaVersion::Lua54, "1125.0"),
            (LuaVersion::Lua55, "1125.0"),
        ],
    );
}

#[test]
fn variable_changes_kind_across_a_generic_for_back_edge() {
    let src = r#"
        local t = {}
        for i = 1, 300 do t[i] = i end
        local x, y = 0.5, 0
        for _, v in ipairs(t) do
          if v % 2 == 1 then y = y + x x = 7 else y = y + x x = 0.5 end
        end
        return tostring(x) .. " " .. tostring(y)"#;
    assert_same(
        src,
        &[
            (LuaVersion::Lua51, "0.5 1125"),
            (LuaVersion::Lua52, "0.5 1125"),
            (LuaVersion::Lua53, "0.5 1125.0"),
            (LuaVersion::Lua54, "0.5 1125.0"),
            (LuaVersion::Lua55, "0.5 1125.0"),
        ],
    );
}

/// A table in one iteration and an integer in the next: restoring the
/// integer's bits as a table would hand the collector a wild pointer.
#[test]
fn variable_changes_between_table_and_integer() {
    let src = r#"
        local x = {}
        local i = 0
        while i < 300 do
          i = i + 1
          if i % 2 == 1 then x = 7 else x = {} end
        end
        collectgarbage()
        return type(x)"#;
    assert_same(src, &all("table"));
}
