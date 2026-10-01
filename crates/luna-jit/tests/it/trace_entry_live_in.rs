//! A frame starts with whatever its register window held before (only
//! missing parameters are cleared), so a register the trace does not
//! read before writing holds different leftovers on each entry. A trace
//! checks only the registers it reads first: it must still be entered,
//! and give the right result, whatever the others hold. A looping trace
//! that writes such a register must not leave a stale value to an exit
//! that the interpreter then reads.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

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

/// Runs `src` with the trace JIT at several thresholds (with and without
/// the method JIT) and compares with the interpreter, whose output must
/// be `want`. Some trace has to have run.
fn assert_same(src: &str, want: &str) {
    let mut bad = Vec::new();
    let mut dispatched = 0;
    for v in VERSIONS {
        let (interp, _) = run(v, src, None);
        assert_eq!(interp, want, "{v:?}: interpreter");
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

/// Calls that leave booleans and tables in the window the next call's
/// frame takes.
const DIRTY: &str = r#"
    local function dirty_bool()
        local a, b, c, d, e, f, g, h = true, false, true, false, true, false, true, false
        return
    end
    local function dirty_tab()
        local a, b, c, d, e, f, g, h = {}, {}, {}, {}, {}, {}, {}, {}
        return
    end
    local function dirty(k)
        if k % 2 == 0 then dirty_bool() else dirty_tab() end
    end
"#;

#[test]
fn traces_enter_whatever_dead_registers_hold() {
    let src = format!(
        "{DIRTY}{}",
        r#"
        local function work(n)
            local s = 0
            for i = 1, n do s = s + i end
            local after = s * 2
            return after
        end
        -- trace `work`'s loop before the driving loop records anything
        local total = work(100) + work(100)
        for k = 1, 60 do
            dirty(k)
            total = total + work(100)
        end
        return total
    "#
    );
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        let r = vm.eval(&src).expect("eval");
        assert!(matches!(r[0], Value::Int(626_200)), "{v:?}: {:?}", r[0]);
        let dispatched = vm.trace_dispatched_count();
        assert!(
            dispatched >= 30,
            "{v:?}: only {dispatched} trace entries (closed {} compiled {} off {:?} fail {:?})",
            vm.jit.counters.closed,
            vm.jit.counters.compiled,
            vm.jit.counters.dispatch_off_reasons,
            vm.jit.counters.compile_failed_reasons
        );
    }
}

#[test]
fn register_read_after_an_exit_before_the_loop_writes_it() {
    // the loop's path writes `x` without reading it, the branch it
    // leaves for reads the value the previous iteration wrote
    let src = format!(
        "{DIRTY}{}",
        r#"
        local function f(n)
            local acc, x = 0, 0
            for i = 1, n do
                if i % 8 == 0 then acc = acc + x end
                x = i
            end
            return acc
        end
        local t = 0
        for k = 1, 30 do dirty(k); t = t + f(100) end
        return tostring(t)
    "#
    );
    assert_same(&src, "18360");
}

#[test]
fn while_loop_temporary_read_after_an_exit() {
    let src = format!(
        "{DIRTY}{}",
        r#"
        local function f(n)
            local i, acc, y = 0, 0, 1
            while i < n do
                i = i + 1
                if i % 5 == 0 then acc = acc + y * 3 end
                y = i + 1
            end
            return acc
        end
        local t = 0
        for k = 1, 30 do dirty(k); t = t + f(60) end
        return tostring(t)
    "#
    );
    assert_same(&src, "35100");
}

#[test]
fn side_path_reads_a_register_the_loop_path_never_touches() {
    let src = format!(
        "{DIRTY}{}",
        r#"
        local function f(n, z)
            local acc = 0
            for i = 1, n do
                if i % 4 == 0 then acc = acc + z else acc = acc + 1 end
            end
            return acc
        end
        local t = 0
        for k = 1, 30 do dirty(k); t = t + f(100, k) end
        return tostring(t)
    "#
    );
    assert_same(&src, "13875");
}

#[test]
fn recursion_enters_with_dirty_windows() {
    let src = format!(
        "{DIRTY}{}",
        r#"
        local function fib(n)
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        local t = 0
        for k = 1, 30 do dirty(k); t = t + fib(12) end
        return tostring(t)
    "#
    );
    assert_same(&src, "4320");
}
