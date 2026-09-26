//! v2.0 Phase 5 Track AO sub-track AO-PF — runtime fire measurement
//! for the Stage 7 polish 6 inline-chain reloc path.
//!
//! NOT a pass/fail test — pure measurement. Prints every relevant
//! probe (`aot_trace_install_count`, `aot_inline_chains_resolved`,
//! `aot_trace_fired pc=`, `trace_materialize_frames_fires`) for a
//! battery of workloads so the verdict doc can cite real numbers,
//! not hypotheses.
//!
//! Workloads:
//! 1. **Inlined helper** (mirrors `stage7_aot_inlined_recursive`):
//!    `for i, inner(i)` — recorder MIGHT inline `inner` into the loop
//!    and emit a cmp@d>0 side-exit. Per stage7 smoke this self-skips
//!    on this build (recorder produces 0 AOT traces for this shape).
//! 2. **Self-recursive fib(28)** — classic self-recursion. The
//!    `is_self_recursive` predicate in `trace.rs:3644` keeps inlined
//!    same-proto Call ops in the trace. Question: does this shape
//!    actually end up with `per_exit_inline.non_empty` on this build?
//! 3. **Self-recursive sum(N)** — same self-recursion shape but
//!    arithmetic instead of branching, so the side-exit profile is
//!    different.
//!
//! Each workload prints all 4 probe lines on stderr. The test PASSES
//! unconditionally (modulo the binary running cleanly + producing the
//! expected stdout); the numbers are for the verdict doc, not gating.

use std::fs;
use std::path::Path;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

fn run_with_env(path: &Path, env_key: &str, env_val: &str) -> (String, String, Option<i32>) {
    let output = Command::new(path)
        .env(env_key, env_val)
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", path.display()));
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

fn measure(label: &str, src: &[u8], expected_stdout: &str) {
    if cfg!(target_os = "windows") {
        eprintln!("[{label}] skipped: AOT trace install unavailable on Windows COFF");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("[{label}] skipped: cc / cargo not on PATH");
        return;
    }

    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join(format!("{label}.lua"));
    fs::write(&src_path, src).expect("write source");

    let out_path = td.path().join(format!("{label}_aot"));
    // Surface harvest diagnostics so the verdict doc can cite the
    // filter accept/reject reason per workload, not just the final
    // installed-trace count.
    // SAFETY: single-threaded test, set_var on a string env key is
    // sound here (cargo test runs each test fn serially within the
    // process unless the test explicitly opts into parallel exec).
    unsafe {
        std::env::set_var("LUNA_AOT_HARVEST_PROBE", "1");
    }
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55).unwrap_or_else(|e| {
        panic!("[{label}] compile_and_link failed: {e}");
    });

    let (stdout, stderr, code) = run_with_env(&out_path, "LUNA_AOT_PROBE", "1");

    eprintln!("========== {label} ==========");
    eprintln!("exit: {code:?}");
    eprintln!("stdout: {stdout:?}");
    eprintln!("stderr:");
    for line in stderr.lines() {
        if line.starts_with("luna-runtime-helpers:") {
            eprintln!("  {line}");
        }
    }
    eprintln!();

    assert_eq!(code, Some(0), "[{label}] binary exited non-zero");
    assert_eq!(stdout, expected_stdout, "[{label}] wrong stdout");
}

#[test]
fn ao_pf_measurement_battery() {
    // Workload 1: inlined helper. Mirrors stage7_aot_inlined_recursive.
    // Expected sum (n = 100000):
    //   1..99 -> 2*i, 100..100000 -> i => 9900 + 5_000_045_050 = 5_000_054_950
    measure(
        "inlined_helper",
        b"local function inner(x)\n\
          if x < 100 then return x * 2 end\n\
          return x\n\
        end\n\
        local s = 0\n\
        for i = 1, 100000 do s = s + inner(i) end\n\
        print(s)\n",
        "5000054950\n",
    );

    // Workload 2: self-recursive fib (small N so the test finishes;
    // fib(20) = 6765, completes in a few hundred ms in interp).
    measure(
        "fib_self_recursive",
        b"local function fib(n)\n\
          if n < 2 then return n end\n\
          return fib(n - 1) + fib(n - 2)\n\
        end\n\
        print(fib(20))\n",
        "6765\n",
    );

    // Workload 3: self-recursive sum (no branch — arithmetic only).
    // sum(1000) = 500500.
    measure(
        "sum_self_recursive",
        b"local function sum(n)\n\
          if n == 0 then return 0 end\n\
          return n + sum(n - 1)\n\
        end\n\
        print(sum(1000))\n",
        "500500\n",
    );

    // Workload 4 (control): hot counted loop. Known to install +
    // dispatch AOT traces (mirrors stage7_aot_trace_fires). Used as
    // positive control to confirm the counter wiring is sound — if
    // this one also reports 0 fires, the counter is broken.
    measure(
        "counted_loop_control",
        b"local s = 0\n\
          for i = 1, 1000000 do s = s + 1 end\n\
          print(s)\n",
        "1000000\n",
    );

    // Workload 5 (control): table-field getfield/setfield loop.
    // Known to install + dispatch (mirrors stage7_aot_recursive_trace).
    measure(
        "getfield_loop_control",
        b"local t = {x = 0}\n\
          for i = 1, 1000000 do t.x = t.x + i end\n\
          print(t.x)\n",
        "500000500000\n",
    );
}
