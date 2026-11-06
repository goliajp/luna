//! The same program run in separate processes with one string hash seed
//! compiles the same traces and leaves them through the same exits every
//! time. Where a table's string keys sit follows the seed, and a trace reads
//! a field in the slot it found the key in while recording, so another seed
//! may give other side traces; nothing but the seed may (no map's iteration
//! order, address or clock).

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

/// Set in a child process: the dialect whose counts it prints.
const CHILD: &str = "LUNA_TEST_TRACE_COUNTS_OF";

/// Writes and reads under string, float and negative keys of a table whose
/// array part grows under them: the rehashes move the keys, the reads'
/// exits start side traces, and those link back to the loop's trace.
const PROGRAM: &str = "
    local function run()
      local u = {0, 0}
      local function f(n)
        local t, s = {}, 0
        for i = 1, n do
          t.a = 1.5 t[1] = 2 t[i] = 0.25 t[2.5] = i t[-3] = 4
          u[i] = i u[1.5] = 2 u.x = i
          s = s + t.a + t[1] + t[i] + t[2.5] + t[-3] + u[i] + u[1.5] + u.x
        end
        return s
      end
      return f(5000)
    end
    local r
    for _ = 1, 4 do r = run() end
    return r";

const SEEDS: [u32; 2] = [1, 2];

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn counts(v: LuaVersion, seed: u32) -> String {
    let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, seed);
    vm.open_all_libs().expect("the libraries open");
    luna_jit::install_default_jit(&mut vm);
    let cl = vm.load(PROGRAM.as_bytes(), b"=counts").expect("compiles");
    let out = vm.call_value(Value::Closure(cl), &[]).expect("runs");
    let mut causes: Vec<String> = (vm.trace_close_cause_counts().iter())
        .map(|(k, n)| format!("{k}={n}"))
        .collect();
    causes.sort();
    format!(
        "{v:?} seed {seed}: {out:?}, compiled {} closed {} aborted {} side traces {}/{} \
         dispatched {} deopts {}, exits {:?}, causes {causes:?}",
        vm.trace_compiled_count(),
        vm.trace_closed_count(),
        vm.trace_aborted_count(),
        vm.trace_side_trace_compiled_count(),
        vm.trace_side_trace_started_count(),
        vm.trace_dispatched_count(),
        vm.trace_deopt_count(),
        vm.trace_exit_hit_summary(cl),
    )
}

/// What a child process runs: nothing unless [`CHILD`] names a dialect.
#[test]
fn counts_of_one_process() {
    let Ok(name) = std::env::var(CHILD) else {
        return;
    };
    let v = DIALECTS
        .into_iter()
        .find(|v| format!("{v:?}") == name)
        .expect("a dialect");
    for seed in SEEDS {
        println!("counts: {}", counts(v, seed));
    }
}

fn counts_in_a_process_of_its_own(v: LuaVersion) -> Vec<String> {
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "--exact",
            "trace_counts_per_process::counts_of_one_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD, format!("{v:?}"))
        .output()
        .expect("the test binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{v:?}: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // the harness prints the test's name on the line its output starts
    let lines: Vec<String> = (stdout.lines())
        .filter_map(|l| l.split_once("counts: ").map(|(_, c)| c.to_string()))
        .collect();
    assert_eq!(lines.len(), SEEDS.len(), "{v:?}: {stdout}");
    lines
}

#[test]
fn separate_processes_with_one_seed_compile_the_same_traces() {
    for v in DIALECTS {
        let here: Vec<String> = SEEDS.iter().map(|&s| counts(v, s)).collect();
        for _ in 0..4 {
            assert_eq!(counts_in_a_process_of_its_own(v), here, "{v:?}");
        }
    }
}
