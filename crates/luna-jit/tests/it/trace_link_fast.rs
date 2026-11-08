//! The optimizing tier's code entering the trace a side trace links to
//! itself, and leaving the link to the Vm for each reason it may: counted
//! by `JitCounters::links_by_vm`, with the interpreter's results.

use luna_core::__private::VmJitState;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// What `src` returns, as text, with the trace JIT on or off; the links
/// taken without the dispatcher, and those the code handed the Vm, by why.
fn run(v: LuaVersion, src: &str, jit: bool) -> (String, u64, [u64; 5]) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let cl = vm.load(src.as_bytes(), b"=link").expect("load");
    let r = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => r
            .iter()
            .map(|v| format!("{v:?}"))
            .collect::<Vec<_>>()
            .join(","),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let c = vm.jit_counters_mut();
    (r, c.links, *c.links_by_vm)
}

/// `src` with the JIT on gives the interpreter's result in every dialect;
/// `check` sees the links taken and those left to the Vm.
fn each(src: &str, check: impl Fn(LuaVersion, u64, [u64; 5])) {
    for v in VERSIONS {
        let (want, ..) = run(v, src, false);
        let (got, links, by_vm) = run(v, src, true);
        assert_eq!(got, want, "{v:?}");
        check(v, links, by_vm);
    }
}

/// The scan loop and the copy loop of the sliding window cell, linked both
/// ways: once both traces are at the optimizing tier the code takes the
/// links itself, round the ring until the links in a row reach their
/// limit, which it leaves to the Vm.
pub(crate) const RING: &str = "
    local total = 0
    for rep = 1, 40 do
      local window, now, horizon = {}, 500, 100
      for i = 1, now do window[i] = i end
      for step = 1, 100 do
        now = now + 1
        window[#window + 1] = now
        local i = 1
        while i <= #window and window[i] < now - horizon do i = i + 1 end
        if i > 1 then
          local new = {}
          for j = i, #window do new[#new + 1] = window[j] end
          window = new
        end
        total = total + #window
      end
    end
    return total";

#[test]
fn the_code_takes_links_and_leaves_those_past_the_limit() {
    each(RING, |v, links, by_vm| {
        let left: u64 = by_vm.iter().sum();
        assert!(
            links > left,
            "{v:?}: {links} links, {by_vm:?} left to the Vm"
        );
        assert!(by_vm[2] > 0, "{v:?}: no link left at the limit: {by_vm:?}");
    });
}

/// A short copy loop entered only through the link, after a long scan
/// loop: the scan loop's trace moves to the optimizing tier long before
/// the copy loop's, and until it does the code leaves the link to the Vm,
/// which counts the copy loop's entries towards its move.
#[test]
fn a_link_into_a_trace_yet_to_move_is_left_to_the_vm() {
    let src = "
        local total = 0
        for rep = 1, 4000 do
          local t = {}
          for i = 1, 60 do t[i] = i end
          local i = 1
          while i <= #t and t[i] < 58 do i = i + 1 end
          for j = i, #t do total = total + t[j] end
        end
        return total";
    each(src, |v, _, by_vm| {
        assert!(
            by_vm[3] > 0,
            "{v:?}: no link left before the move: {by_vm:?}"
        );
    });
}

/// The sliding window ring as a function, run twice on one Vm: the second
/// time with the run words holding that hot counting pauses, so that the
/// code, its traces at the optimizing tier by then, leaves each link to
/// the Vm. The results, and the links left for that reason in each run.
fn ring_held_quiet(v: LuaVersion, jit: bool) -> ([String; 2], [u64; 2]) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let src = format!("return function() {RING} end");
    let ring = vm.eval(&src).expect("ring")[0];
    vm.pin_host(ring);
    let mut out = [String::new(), String::new()];
    let mut quiet = [0; 2];
    for (k, hold) in [false, true].into_iter().enumerate() {
        luna_core::__private::hold_run_words_quiet(&vm, hold);
        let before = vm.jit_counters_mut().links_by_vm[1];
        out[k] = format!("{:?}", vm.call_value(ring, &[]).expect("ring runs"));
        quiet[k] = vm.jit_counters_mut().links_by_vm[1] - before;
    }
    luna_core::__private::hold_run_words_quiet(&vm, false);
    (out, quiet)
}

#[test]
fn a_link_reached_while_hot_counting_pauses_is_left_to_the_vm() {
    for v in VERSIONS {
        let (want, _) = ring_held_quiet(v, false);
        let (got, quiet) = ring_held_quiet(v, true);
        assert_eq!(got, want, "{v:?}");
        assert_eq!(quiet[0], 0, "{v:?}: a link left for a pause none held");
        assert!(quiet[1] > 0, "{v:?}: no link left while the pause was held");
    }
}

/// The sliding window ring with a call inlined in each loop: the copy
/// loop's trace inlines `f`, and the scan loop's inlines `pad`, whose
/// locals make its window wider than the copy loop's so that the link
/// into the copy loop is filled. The code leaves a link into a trace that
/// inlines calls to the Vm, which knows whether the thread has room for
/// the frames those calls need.
#[test]
fn a_link_into_a_trace_that_inlines_calls_is_left_to_the_vm() {
    let src = "
        local function pad(x)
          local x1, x2, x3, x4, x5, x6, x7, x8, x9, x10 = 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
          local y1, y2, y3, y4, y5, y6, y7, y8, y9, y10 = 1, 2, 3, 4, 5, 6, 7, 8, 9, 10
          return 1
        end
        local function f(x) return x end
        local total = 0
        for rep = 1, 40 do
          local window, now, horizon = {}, 500, 100
          for i = 1, now do window[i] = i end
          for step = 1, 100 do
            now = now + 1
            window[#window + 1] = now
            local i = 1
            while i <= #window and window[i] < now - horizon do i = i + pad(i) end
            if i > 1 then
              local new = {}
              for j = i, #window do new[#new + 1] = f(window[j]) end
              window = new
            end
            total = total + #window
          end
        end
        return total";
    each(src, |v, _, by_vm| {
        assert!(
            by_vm[4] > 0,
            "{v:?}: no link into an inlining trace left to the Vm: {by_vm:?}"
        );
    });
}
