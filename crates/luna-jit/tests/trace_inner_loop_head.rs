//! A trace recorded from an inner loop's head can run out of the inner
//! loop and close at the outer loop's back edge. The outer back edge
//! continues at the outer body, which is not the trace's head: looping
//! straight back to the head skipped the outer body code before the
//! inner loop (here the `w = 0` that restarts it), so the inner loop
//! stopped running and the outer loop finished early.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, hot: Option<u32>) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    match hot {
        Some(h) => {
            vm.jit.trace_hot_threshold = h;
            vm.jit.call_hot_threshold = h;
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

#[track_caller]
fn same(src: &str, want: &str) {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let (interp, _) = run(v, src, None);
        assert_eq!(interp, want, "{v:?}: interpreter");
        let mut dispatched = 0;
        for hot in 1..=5 {
            let (jit, d) = run(v, src, Some(hot));
            assert_eq!(jit, interp, "{v:?}, hot {hot}: JIT differs from the interpreter");
            dispatched += d;
        }
        assert!(dispatched > 0, "{v:?}: no trace was dispatched");
    }
}

#[test]
fn while_inside_numeric_for() {
    same(
        r#"
        local n = 0
        for i = 1, 40 do
          local w = 0
          while w < 1 do w = w + 1 n = n + 1 end
        end
        return tostring(n)"#,
        "40",
    );
}

#[test]
fn while_inside_ipairs() {
    same(
        r#"
        local a = {}
        for i = 1, 40 do a[i] = i end
        local n = 0
        for _, v in ipairs(a) do
          local w = 0
          while w < 1 do w = w + 1 n = n + v end
        end
        return tostring(n)"#,
        "820",
    );
}
