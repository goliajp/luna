//! An error raised while a trace is being recorded ends that recording. A
//! function-entry recording whose call raised (caught by `pcall`) used to
//! close when the next call reached the head again: the trace held the body
//! only up to the failing op and returned to its head, so every later call
//! spun between the trace and the dispatcher for ever.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;
use std::sync::mpsc;
use std::time::Duration;

const SRC: &str = r#"
    local seq, c = {}, {0}
    local t = {1, 2, 3}
    local objs = {t, nil, t}
    local x
    local w = 0
    while w < 100 do
      w = w + 1
      pcall(function()
        seq[#seq + 1] = (- c[1]) - objs[w % 3 + 1][w % 8 + 1]
        x = false
      end)
    end
    return tostring(#seq) .. " " .. tostring(x)"#;

/// Runs `SRC` on a thread of its own; `None` if it has not finished within
/// a few seconds (the spinning thread is left behind).
fn run(method: bool, trace_hot: u32, call_hot: u32) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
        vm.set_jit_enabled(method);
        vm.set_trace_jit_enabled(true);
        vm.jit.trace_hot_threshold = trace_hot;
        vm.jit.call_hot_threshold = call_hot;
        let out = match vm.eval(SRC).expect("eval").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        };
        let _ = tx.send(out);
    });
    rx.recv_timeout(Duration::from_secs(20)).ok()
}

#[test]
fn error_inside_a_recording_does_not_close_it() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    let interp = match vm.eval(SRC).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    };
    assert_eq!(interp, "25 false");
    for (method, trace_hot, call_hot) in [(false, 16, 1), (false, 2, 1), (true, 2, 3), (true, 1, 1)]
    {
        assert_eq!(
            run(method, trace_hot, call_hot).as_deref(),
            Some(interp.as_str()),
            "method JIT {method}, trace hot {trace_hot}, call hot {call_hot}"
        );
    }
}
