//! P15-A v0 — `HotExitInfo` + `Vm::hot_exit_iter` accessor.
//!
//! Foundation for side trace tree: the walker surfaces every
//! `exit_hit_counts` slot whose hit count crosses
//! `HOTEXIT_THRESHOLD = 10` (LuaJIT 2.1 default). Per-exit data was
//! shipped in `e52e67a` (P15-prep); this commit only exposes the
//! detection layer — no recording, no side trace compile yet.
//!
//! Probe-verified targets (from P15-prep ship message):
//! - fib_28:          5 nonzero exits, idx 8 = 237877 hits (55%)
//! - binary_trees_d4: 4 nonzero exits, idx 6 = 988 hits (42%)
//! - concat_str_for_10k: 1 dispatch total → 1 exit hit, BELOW threshold
//!
//! The first two MUST surface; the third MUST NOT.

use luna_jit::jit::trace::HOTEXIT_THRESHOLD;
use luna_jit::version::LuaVersion;

/// fib(28) — the trace's hottest side-exit (idx 8) crosses
/// threshold many times over. `hot_exit_iter` must surface at least
/// that one entry, and every reported entry must satisfy
/// `hits >= HOTEXIT_THRESHOLD`.
#[test]
fn fib_28_hot_exit_surfaces() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local function f(n)
                  if n < 2 then return n end
                  return f(n-1) + f(n-2)
              end
              return f(28)",
            b"=fib28",
        )
        .unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(317811)));

    let hot = vm.hot_exit_iter(cl);
    assert!(
        !hot.is_empty(),
        "fib_28 must surface at least one hot side-exit; got 0"
    );
    for info in &hot {
        assert!(
            info.hits >= HOTEXIT_THRESHOLD,
            "hot_exit_iter reported sub-threshold entry: idx={} hits={}",
            info.exit_idx,
            info.hits
        );
        assert!(
            !info.exit_tags.is_empty(),
            "exit_tags must be populated; idx={}",
            info.exit_idx
        );
    }
    // The dominant exit per P15-prep probe is in the fib inner
    // proto's trace, NOT the outer chunk's. The walker recurses, so
    // it must reach the inner proto's trace.
    let max_hits = hot.iter().map(|h| h.hits).max().unwrap();
    assert!(
        max_hits >= 100_000,
        "fib_28 hottest exit should report ≥100k hits (P15-prep probe \
         saw 237k at idx 8); got max_hits={}",
        max_hits
    );
}

/// binary_trees_d4 — at least one hot side-exit must surface and
/// satisfy the threshold predicate. Per P15-prep probe, idx 6 hits
/// 988× across 2371 dispatches.
#[test]
fn binary_trees_d4_hot_exit_surfaces() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local function make(d)
                  if d == 0 then return 1 end
                  return 1 + make(d-1) + make(d-1)
              end
              local s = 0
              for i = 1, 200 do s = s + make(4) end
              return s",
            b"=btrees_d4",
        )
        .unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    // make(4) = 1 + 2 * make(3) = 1 + 2*(1 + 2*(1 + 2*(1 + 2))) = 31
    // sum = 200 * 31 = 6200
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(6200)));

    let hot = vm.hot_exit_iter(cl);
    assert!(
        !hot.is_empty(),
        "binary_trees_d4 must surface at least one hot side-exit; got 0"
    );
    for info in &hot {
        assert!(
            info.hits >= HOTEXIT_THRESHOLD,
            "hot_exit_iter reported sub-threshold entry: idx={} hits={}",
            info.exit_idx,
            info.hits
        );
    }
}

/// concat_str_for_10k — the body's trace dispatches exactly once
/// (the outer for loop's back-edge fires once, then the string
/// length grows and entry tags re-check fails). Every exit slot's
/// hit count is at most 1, well below threshold; `hot_exit_iter`
/// must return an empty vector.
#[test]
fn concat_str_for_10k_no_hot_exit() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local t = {}
              for i = 1, 10000 do t[i] = 'x' end
              local function join(tt)
                  local s = ''
                  for _, v in ipairs(tt) do s = s..v end
                  return s
              end
              return string.len(join(t))",
            b"=concat_str",
        )
        .unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(10000)));

    let hot = vm.hot_exit_iter(cl);
    assert!(
        hot.is_empty(),
        "concat_str_for_10k must NOT surface any hot side-exit \
         (single-dispatch trace, no exit reaches threshold); got {} \
         entries: {:?}",
        hot.len(),
        hot.iter().map(|h| (h.exit_idx, h.hits)).collect::<Vec<_>>()
    );
}
