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
