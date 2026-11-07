//! Side traces that end at the head of another trace of their parent's
//! frame: the parent's code enters that trace without the dispatcher.

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
/// taken without the dispatcher, the side cells filled with a link, and the
/// links the dispatcher took once the code had taken its most in a row.
fn run(v: LuaVersion, src: &str, jit: bool) -> (String, u64, u64, u64) {
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
    (r, c.links, c.relink[6], c.links_redispatched)
}

/// A scan loop and a copy loop, each left for the other through a side
/// trace (the shape of the sliding window cell): the side traces link the
/// two loops' traces both ways, a ring, with the interpreter's results.
#[test]
fn two_loops_linked_both_ways() {
    let src = "
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
    for v in VERSIONS {
        let (want, ..) = run(v, src, false);
        let (got, links, filled, again) = run(v, src, true);
        assert_eq!(got, want, "{v:?}");
        assert!(filled > 0, "{v:?}: no side cell holds a link");
        assert!(links > 0, "{v:?}: no link taken");
        // round the ring the links nest, one native frame each, until the
        // code leaves the next to the dispatcher
        assert!(
            again > 0,
            "{v:?}: the links in a row never reached their limit"
        );
        assert!(
            again <= links / 8,
            "{v:?}: {again} of {links} links taken again"
        );
    }
}

/// Two traces linked to each other, a ring through their side cells, are
/// freed with the Vm: the cells do not hold the traces they link to.
#[test]
fn traces_linked_in_a_ring_are_freed_with_the_vm() {
    use luna_jit::__private::ProtoTraceInternals;
    use luna_jit::jit::send_compat::TArc;
    let src = "
        local total = 0
        for rep = 1, 40 do
          local t = {}
          for i = 1, 300 do t[i] = i end
          local i = 1
          while i <= #t and t[i] < 200 do i = i + 1 end
          for j = i, #t do total = total + t[j] end
        end
        return total";
    for v in VERSIONS {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(true);
        let cl = vm.load(src.as_bytes(), b"=ring").expect("load");
        vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(vm.jit_counters_mut().links > 0, "{v:?}: no link taken");
        let traces: Vec<_> = cl.proto.traces().iter().map(TArc::downgrade).collect();
        assert!(traces.len() >= 2, "{v:?}: {} traces", traces.len());
        drop(vm);
        let alive = traces.iter().filter(|w| w.upgrade().is_some()).count();
        assert_eq!(alive, 0, "{v:?}: traces outlive the Vm");
    }
}

/// The sliding window cell, loaded twice into one Vm: the second load takes
/// the first one's traces over, the scan loop's side trace before the copy
/// loop's trace, so the side trace's cell is left empty when it is wired;
/// once the copy loop's trace is cached the cell links to it.
#[test]
fn a_side_trace_wired_before_the_trace_it_ends_at_links_once_that_is_cached() {
    let src = "
        local window, now, horizon, kept = {}, 500, 100, 0
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
          kept = #window
        end
        return kept";
    for v in VERSIONS {
        let (want, ..) = run(v, src, false);
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(true);
        for _ in 0..2 {
            let cl = vm.load(src.as_bytes(), b"=late").expect("load");
            let got = vm.call_value(Value::Closure(cl), &[]).expect("run");
            let got = got
                .iter()
                .map(|v| format!("{v:?}"))
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(got, want, "{v:?}");
        }
        assert!(vm.trace_adopted_count() > 0, "{v:?}: nothing taken over");
        let c = vm.jit_counters_mut();
        assert_eq!(c.relink[3], 0, "{v:?}: cells left empty: {:?}", c.relink);
        assert!(c.relink[6] > 0 && c.links > 0, "{v:?}: {:?}", c.relink);
    }
}

/// What `src` returns, as text, with the trace JIT on or off, and the side
/// cell fills by reason.
fn run_fills(v: LuaVersion, src: &str, jit: bool) -> (String, [u64; 8]) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    let cl = vm.load(src.as_bytes(), b"=nolink").expect("load");
    let r = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => format!("{r:?}"),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (r, vm.jit_counters_mut().relink)
}

/// A side trace from a loop's last exit that ends where its function
/// returns ends at no loop head: its cell is counted as having no trace to
/// link to (7), not as one whose loop head has no trace yet (3).
#[test]
fn a_side_trace_ending_at_no_loop_head_is_counted_apart() {
    let bodies = [
        "local n = 0
         for i = 1, 300 do n = n + i end
         return n",
        "local n, buf = 0, ''
         for i = 1, 300 do
           local k = 'item:' .. i .. ':' .. (i % 7)
           n = n + #k
           buf = buf .. k
           if #buf > 600 then buf = '' end
         end
         return n + #buf",
    ];
    for (s, body) in bodies.iter().enumerate() {
        let src = format!(
            "local function f() {body} end
             local total = 0
             for k = 1, 60 do total = total + f() end
             return total"
        );
        for v in VERSIONS {
            let (want, _) = run_fills(v, &src, false);
            let (got, fills) = run_fills(v, &src, true);
            assert_eq!(got, want, "{v:?} shape {s}");
            assert!(fills[7] > 0, "{v:?} shape {s}: {fills:?}");
            assert_eq!(fills[3], 0, "{v:?} shape {s}: {fills:?}");
        }
    }
}
