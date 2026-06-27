//! v2.0 Stage 7 polish 6 — inline-side-exit runtime fire experiment.
//!
//! Runs several Lua source patterns and reports for each:
//!   - whether the trace recorder produced any closed traces
//!   - the recorder/lowerer close-cause taxonomy and `dispatch_off`
//!     reason histogram
//!
//! Goal of the experiment: identify a Lua source pattern that fires
//! the conjunction `dispatchable == true AND per_exit_inline.non_empty()`.
//! The polish 6 chain-reloc + deploy resolver path only exercises when
//! BOTH conditions hold. As of v2.0 R1+ all `SelfLink` closes pin
//! `dispatchable = false` (`"self-link-retf-r1"`), `DownRec` closes
//! pin (`"downrec-stitch-pending"`), and any depth>0 op whose `proto !=
//! head_proto` pins (`"InlineAbort-gate"`) — see
//! `crates/luna-jit/src/jit_backend/trace.rs:7592 / 7536 / 7964`.
//!
//! Run: `cargo run --example diag_polish6_inline_fire --release -p luna-jit`
//!
//! Interpretation: a row whose `dispatch_off_reasons` is empty AND
//! `trace_compiled > 0` would indicate a dispatchable trace; correlate
//! against the corresponding source pattern. The harvest probe in
//! `luna-aot::embed::compile_and_link` (gated on
//! `LUNA_AOT_HARVEST_PROBE`) is the AOT-side complement: it prints
//! `accepted_with_per_exit_inline` for installable traces, which is
//! the eventual fire-or-not gate the polish 6 deploy resolver depends
//! on.

use luna_jit::version::LuaVersion;

const PATTERN_INLINED_HELPER: &str = r#"
    local function inner(x)
      if x < 100 then return x * 2 end
      return x
    end
    local s = 0
    for i = 1, 100000 do s = s + inner(i) end
    return s
"#;

const PATTERN_SHALLOW_SELF_REC: &str = r#"
    local function f(n)
      if n > 0 then
        return 1 + f(n - 1)
      end
      return 0
    end
    -- Recursion depth = 1 stays below RECUNROLL_THRESHOLD + 1; the
    -- self-link cycle catch should NOT trip. Hot-loop calls drive the
    -- call-trigger hot counter past `CALL_HOT_THRESHOLD`.
    local s = 0
    for i = 1, 100000 do s = s + f(1) end
    return s
"#;

const PATTERN_TWO_LEVEL_SELF_REC: &str = r#"
    local function f(n)
      if n > 0 then
        return 1 + f(n - 1)
      end
      return 0
    end
    local s = 0
    for i = 1, 100000 do s = s + f(2) end
    return s
"#;

const PATTERN_FIB28: &str = r#"
    local function fib(n)
      if n < 2 then return n end
      return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
"#;

const PATTERN_INNER_FOR_LOOP: &str = r#"
    local function inner(n)
      local s = 0
      for i = 1, n do s = s + i end
      return s
    end
    local total = 0
    for j = 1, 10000 do total = total + inner(10) end
    return total
"#;

const PATTERN_INLINED_HELPER_TIGHT: &str = r#"
    -- Inner has only `if` + return (no arith result-mix); produces a
    -- depth>0 cmp side-exit shape with the smallest possible body.
    local function pick(x)
      if x < 50000 then return 1 end
      return 2
    end
    local s = 0
    for i = 1, 100000 do s = s + pick(i) end
    return s
"#;

fn run_pattern(label: &str, src: &str) {
    run_pattern_at(label, src, /*chunk_jit=*/ false);
}

fn run_pattern_chunk_jit_on(label: &str, src: &str) {
    run_pattern_at(label, src, /*chunk_jit=*/ true);
}

fn run_pattern_at(label: &str, src: &str, chunk_jit: bool) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    // Default: chunk-compiler JIT off so the trace recorder sees Call
    // sites. `chunk_jit = true` mirrors the AOT harvest VM
    // (`luna-aot::embed::compile_and_link` calls
    // `new_with_jit` which leaves `jit_enabled = true`), so we can see
    // whether the same pattern fires under AOT-harvest conditions.
    vm.set_jit_enabled(chunk_jit);
    vm.set_trace_jit_enabled(true);
    vm.open_base();
    vm.open_math();

    let r = vm.eval(src);
    let returned = match &r {
        Ok(vs) => vs.first().cloned(),
        Err(e) => {
            println!("== {label} ==");
            println!("  eval ERROR: {:?}", e);
            return;
        }
    };

    println!("== {label} (chunk_jit = {chunk_jit}) ==");
    println!("  returned:               {:?}", returned);
    println!("  trace_closed:           {}", vm.trace_closed_count());
    println!("  trace_aborted:          {}", vm.trace_aborted_count());
    println!("  trace_compiled:         {}", vm.trace_compiled_count());
    println!(
        "  trace_compile_failed:   {}",
        vm.trace_compile_failed_count()
    );
    println!("  trace_dispatched:       {}", vm.trace_dispatched_count());
    println!("  trace_deopt:            {}", vm.trace_deopt_count());
    println!(
        "  trace_inline_abort:     {}",
        vm.trace_inline_abort_count()
    );
    println!(
        "  materialize_emit:       {}",
        vm.trace_materialize_emit_count()
    );
    println!(
        "  per_exit_inline_compiled:     {}",
        vm.trace_per_exit_inline_compiled_count()
    );
    println!(
        "  per_exit_inline_dispatchable: {}    <-- polish 6 fire gate",
        vm.trace_per_exit_inline_dispatchable_count()
    );

    if !vm.trace_closed_lens().is_empty() {
        println!("  closed_lens (n = {}):", vm.trace_closed_lens().len());
        for (is_call, n) in vm.trace_closed_lens() {
            println!("    - is_call_triggered = {}, ops_len = {}", is_call, n);
        }
    }

    if !vm.trace_dispatch_off_reasons().is_empty() {
        let mut by_reason: std::collections::BTreeMap<&str, u64> =
            std::collections::BTreeMap::new();
        for r in vm.trace_dispatch_off_reasons() {
            *by_reason.entry(*r).or_insert(0) += 1;
        }
        println!("  dispatch_off_reasons:");
        for (r, c) in by_reason.iter() {
            println!("    - {} : {}", r, c);
        }
    } else {
        println!("  dispatch_off_reasons:   (empty — every compiled trace stayed dispatchable)");
    }

    if !vm.trace_compile_failed_reasons().is_empty() {
        let mut by_reason: std::collections::BTreeMap<&str, u64> =
            std::collections::BTreeMap::new();
        for r in vm.trace_compile_failed_reasons() {
            *by_reason.entry(*r).or_insert(0) += 1;
        }
        println!("  compile_failed_reasons:");
        for (r, c) in by_reason.iter() {
            println!("    - {} : {}", r, c);
        }
    }

    let close_cause = vm.trace_close_cause_counts();
    if !close_cause.is_empty() {
        let sorted: std::collections::BTreeMap<&str, u64> =
            close_cause.iter().map(|(k, v)| (*k, *v)).collect();
        println!("  close_cause_counts:");
        for (r, c) in sorted.iter() {
            println!("    - {} : {}", r, c);
        }
    }
    println!();
}

fn main() {
    println!("# v2.0 Stage 7 polish 6 inline-side-exit fire experiment");
    println!("#");
    println!("# Each row runs a Lua pattern with the chunk-compiler JIT off");
    println!("# and the trace JIT on. The polish 6 code path (chain reloc +");
    println!("# deploy resolver) only fires when a compiled trace has BOTH");
    println!("# `dispatchable == true` AND `per_exit_inline.non_empty()`.");
    println!("#");
    println!("# Compiled-and-no-dispatch_off_reason row would be the win;");
    println!("# any other shape documents a recorder/lowerer gate.");
    println!();
    run_pattern(
        "P1: inlined helper (mirrors stage7_aot smoke)",
        PATTERN_INLINED_HELPER,
    );
    run_pattern("P2: shallow self-rec (depth = 1)", PATTERN_SHALLOW_SELF_REC);
    run_pattern(
        "P3: two-level self-rec (depth = 2)",
        PATTERN_TWO_LEVEL_SELF_REC,
    );
    run_pattern("P4: fib(28) (deep self-rec)", PATTERN_FIB28);
    run_pattern("P5: inner-with-for-loop hot caller", PATTERN_INNER_FOR_LOOP);
    run_pattern(
        "P6: tight inlined helper (smallest depth>0 cmp shape)",
        PATTERN_INLINED_HELPER_TIGHT,
    );

    // Patterns whose helper body is OUTSIDE the chunk-compiler int-
    // arith whitelist. The helper proto fails `populate_jit_cache` so
    // `try_jit_call_op` always bails to `push_frame`, the recorder
    // engages, and the trace shape ends up the same as P2/P3/P4 with
    // chunk_jit=false — even under chunk_jit=true (the AOT harvest
    // default). Goal: a pattern that fires polish 6 under harvest
    // conditions.
    let p_string_helper = r#"
        local concat = ""
        local function f(n)
          -- string ops are NOT in the chunk-compiler int-arith
          -- whitelist; populate_jit_cache returns Skipped → Failed,
          -- try_jit_call_op bails to push_frame.
          local tag = "x"
          if n > 0 then
            return 1 + f(n - 1)
          end
          return 0
        end
        local s = 0
        for i = 1, 100000 do s = s + f(1) end
        return s
    "#;
    let p_table_helper = r#"
        local t = {}
        local function f(n)
          -- table access in the body. Same whitelist-bypass intent.
          if n > 0 then
            return 1 + f(n - 1)
          end
          t[1] = n
          return 0
        end
        local s = 0
        for i = 1, 100000 do s = s + f(1) end
        return s
    "#;
    run_pattern_chunk_jit_on("P7H: string-bearing helper, chunk_jit ON", p_string_helper);
    run_pattern_chunk_jit_on("P8H: table-bearing helper, chunk_jit ON", p_table_helper);

    println!("# ===== chunk_jit = true (mirror AOT harvest VM) =====");
    println!("# `luna-aot::embed::compile_and_link` uses `new_with_jit`");
    println!("# which leaves chunk-compiler JIT on. `try_jit_call_op`");
    println!("# (`exec.rs:1519`) short-circuits int-arith-whitelist body");
    println!("# calls BEFORE `push_frame` — the site that bumps the call");
    println!("# trigger's hot counter. So a workload that worked above");
    println!("# may produce 0 traces under harvest conditions.");
    println!();
    run_pattern_chunk_jit_on(
        "P2H: shallow self-rec, chunk_jit ON",
        PATTERN_SHALLOW_SELF_REC,
    );
    run_pattern_chunk_jit_on(
        "P3H: two-level self-rec, chunk_jit ON",
        PATTERN_TWO_LEVEL_SELF_REC,
    );
    run_pattern_chunk_jit_on("P4H: fib(28), chunk_jit ON", PATTERN_FIB28);
}
