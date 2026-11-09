//! The baseline tier's call stub taking the link a side trace ends at
//! itself, from the words its cell holds, and handing it to the Vm for each
//! reason it may: counted by `JitCounters::links_by_stub` and
//! `links_left_by_stub`, with the results, the links and the moves to the
//! optimizing tier the Vm's way gives.

use luna_core::__private::VmJitState;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

use super::trace_link_fast::RING;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// What a run gives and counts.
#[derive(Debug, PartialEq)]
struct Ran {
    result: String,
    links: u64,
    tiered_up: u64,
    dispatched: u64,
    by_stub: u64,
    left: [u64; 7],
}

/// What a test holds up in the run words through the run.
#[derive(Clone, Copy, PartialEq)]
enum Hold {
    Nothing,
    /// The blocked word, as when an exit left the dispatcher something.
    Blocked,
    /// The note that hot counting pauses.
    Quiet,
}

/// How a run goes besides its source: the stub taking links or not, what
/// the run words hold up, and the count a trace moves to the optimizing
/// tier at (`None`: the default).
#[derive(Clone, Copy)]
struct How {
    stub: bool,
    hold: Hold,
    at: Option<u32>,
}

const PLAIN: How = How {
    stub: true,
    hold: Hold::Nothing,
    at: None,
};

/// `src` run with the trace JIT on or off (`jit`), as `how` says.
fn run(v: LuaVersion, src: &str, jit: bool, how: How) -> Ran {
    // one hash seed and one length seed: runs to compare do the same work
    luna_core::__private::set_len_search_seed(Some(1));
    let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, 1);
    luna_jit::install_default_jit(&mut vm);
    vm.open_all_libs().expect("the libraries open");
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    if let Some(at) = how.at {
        vm.set_trace_tier_up_at(at);
    }
    luna_core::__private::let_stub_take_links(&vm, how.stub);
    luna_core::__private::hold_run_words_quiet(&vm, how.hold == Hold::Quiet);
    luna_core::__private::hold_run_words_blocked(&vm, how.hold == Hold::Blocked);
    let cl = vm.load(src.as_bytes(), b"=link").expect("load");
    let result = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => format!("{r:?}"),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    luna_core::__private::hold_run_words_quiet(&vm, false);
    luna_core::__private::hold_run_words_blocked(&vm, false);
    let c = vm.jit_counters_mut();
    Ran {
        result,
        links: c.links,
        tiered_up: c.tiered_up,
        dispatched: c.dispatched,
        by_stub: c.links_by_stub,
        left: c.links_left_by_stub,
    }
}

/// `src` run with the stub taking links and kept from it, `how` otherwise:
/// both give the interpreter's result, take the same links, move the same
/// traces and dispatch as often; the run with the stub taking them.
fn either_way(v: LuaVersion, src: &str, how: How) -> Ran {
    let want = run(v, src, false, PLAIN).result;
    let on = run(v, src, true, How { stub: true, ..how });
    let off = run(v, src, true, How { stub: false, ..how });
    assert_eq!(on.result, want, "{v:?}");
    assert_eq!(
        off.by_stub, 0,
        "{v:?}: the stub took links kept off: {off:?}"
    );
    assert_eq!(off.left[1..], [0; 6], "{v:?}: {off:?}");
    let same = |r: &Ran| (r.result.clone(), r.links, r.tiered_up, r.dispatched);
    assert_eq!(same(&on), same(&off), "{v:?}: {on:?} {off:?}");
    on
}

#[test]
fn the_stub_takes_links_as_the_vm_would() {
    for v in VERSIONS {
        let on = either_way(v, RING, PLAIN);
        assert!(on.by_stub > 0, "{v:?}: the stub took no link: {on:?}");
    }
}

/// The ring links its two loops in a row until the links reach their
/// limit, and the first link of a Vm finds its trail with no room yet.
#[test]
fn the_stub_leaves_links_at_the_limit_and_with_the_trail_full() {
    for v in VERSIONS {
        let on = run(v, RING, true, PLAIN);
        assert!(on.left[3] > 0, "{v:?}: none left at the limit: {on:?}");
        assert!(
            on.left[4] > 0,
            "{v:?}: none left with the trail full: {on:?}"
        );
    }
}

/// With the blocked word or the pause note held up the stub takes no link
/// and hands each to the Vm for that reason; the Vm, which reads neither
/// (nothing is parked, and a pause it ends leaves the held note up), takes
/// it.
#[test]
fn the_stub_leaves_links_while_the_run_words_hold_it_back() {
    for (hold, why) in [(Hold::Blocked, 1), (Hold::Quiet, 2)] {
        let how = How { hold, ..PLAIN };
        for v in VERSIONS {
            let want = run(v, RING, false, PLAIN).result;
            let on = run(v, RING, true, how);
            assert_eq!(on.result, want, "{v:?}");
            assert_eq!(on.by_stub, 0, "{v:?}: a link taken held back: {on:?}");
            assert!(
                on.left[why] > 0,
                "{v:?}: none left for reason {why}: {on:?}"
            );
            assert!(on.links > 0, "{v:?}: the Vm took no link: {on:?}");
        }
    }
}

/// `x` is a float on every third pass and an integer otherwise: the scan
/// loop never reads it, the copy loop it links to checks it on entry, so
/// the link checks it on the stack. From 5.3 on the two kinds have tags of
/// their own, and the passes with a float fail that check.
#[test]
fn the_stub_leaves_links_whose_slot_check_fails() {
    let src = "
        local total = 0
        for rep = 1, 400 do
          local x = (rep % 3 == 0) and 0.5 or 1
          local t = {}
          for i = 1, 60 do t[i] = i end
          local i = 1
          while i <= #t and t[i] < 30 do i = i + 1 end
          for j = i, #t do total = total + t[j] * x end
        end
        return total";
    for v in VERSIONS {
        let on = either_way(v, src, PLAIN);
        if v >= LuaVersion::Lua53 {
            assert!(on.left[5] > 0, "{v:?}: no check failed: {on:?}");
        }
    }
}

/// The ring as a function called again and again, its traces moving late:
/// past the first call a trace moves at a quarter of its count, which only
/// an entry checks, so the first link into the copy loop in a later call
/// finds its move due. The Vm makes it, at the entry it makes it at
/// without the stub.
#[test]
fn the_stub_leaves_links_whose_move_is_due() {
    let src = "
        local function ring()
          local total = 0
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
          return total
        end
        local total = 0
        for k = 1, 40 do total = total + ring() end
        return total";
    let how = How {
        at: Some(160_000),
        ..PLAIN
    };
    for v in VERSIONS {
        let on = either_way(v, src, how);
        assert!(on.left[6] > 0, "{v:?}: no link found its move due: {on:?}");
        assert!(on.tiered_up > 0, "{v:?}: {on:?}");
    }
}

/// The ring with a call inlined in each loop (as in `trace_link_fast`):
/// the copy loop's trace inlines `f`, and the room for its frame only the
/// Vm knows, so the cell holds no words for the stub, which hands each
/// link into it to the Vm.
#[test]
fn the_stub_leaves_links_it_has_no_words_for() {
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
    for v in VERSIONS {
        let on = either_way(v, src, PLAIN);
        assert!(
            on.left[0] > 0,
            "{v:?}: no link left for want of words: {on:?}"
        );
    }
}
