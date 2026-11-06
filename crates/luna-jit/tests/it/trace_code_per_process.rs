//! The same program run in separate processes with one string hash seed
//! lowers its traces to the same instructions every time. Each process
//! seeds its hash maps differently, so nothing the lowerer emits may follow
//! a hash map's iteration order: the order of the writes at a loop's join
//! decides the register assignment, and with it the machine code.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

/// Set in a child process: the dialect it runs the program in.
const CHILD: &str = "LUNA_TEST_TRACE_CODE_OF";

/// Loops whose first ops read several upvalues into temporaries, so the
/// loop head's copy of those ops joins the first pass's copy through the
/// upvalue reads of both.
const PROGRAM: &str = "
    local a, b, c, d, e, g, h = 1, 2, 3, 4, 5, 6, 7
    local function f(n)
      local s = 0
      for i = 1, n do
        local x = a + b + g
        local y = c + d + e + h
        s = s + x * y + i
      end
      return s
    end
    local function k(n)
      local s = 0
      for i = 1, n do
        local fn = function() return a + b + c end
        local q = d * e
        s = s + fn() + q
      end
      return s
    end
    local r1, r2
    for _ = 1, 3 do r1 = f(20000) r2 = k(20000) end
    return r1, r2";

/// The dialects the baseline tier compiles these loops in: under 5.1 and
/// 5.2 no trace of them is compiled.
const DIALECTS: [LuaVersion; 3] = [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];

/// What a child process runs: nothing unless [`CHILD`] names a dialect.
/// The parent sets `LUNA_BASELINE_DUMP`, which the compiled traces'
/// instructions are written to.
#[test]
fn code_of_one_process() {
    let Ok(name) = std::env::var(CHILD) else {
        return;
    };
    let v = DIALECTS
        .into_iter()
        .find(|v| format!("{v:?}") == name)
        .expect("a dialect");
    let mut vm = luna_jit::vm::Vm::new_minimal_with_hash_seed(v, 1);
    vm.open_all_libs().expect("the libraries open");
    luna_jit::install_default_jit(&mut vm);
    let cl = vm.load(PROGRAM.as_bytes(), b"=code").expect("compiles");
    vm.call_value(Value::Closure(cl), &[]).expect("runs");
}

/// The instruction listings a child process wrote, in trace order.
fn code_in_a_process_of_its_own(v: LuaVersion, run: u32) -> Vec<String> {
    let dir = std::env::temp_dir().join(format!(
        "luna-trace-code-{}-{v:?}-{run}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the dump directory");
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            "--exact",
            "trace_code_per_process::code_of_one_process",
            "--test-threads=1",
        ])
        .env(CHILD, format!("{v:?}"))
        .env("LUNA_BASELINE_DUMP", &dir)
        .output()
        .expect("the test binary runs");
    assert!(
        out.status.success(),
        "{v:?}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut names: Vec<String> = (std::fs::read_dir(&dir).expect("the dump directory"))
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|n| n.ends_with(".txt"))
        .collect();
    names.sort();
    let listings = names
        .iter()
        .map(|n| std::fs::read_to_string(dir.join(n)).expect("a listing"))
        .collect();
    std::fs::remove_dir_all(&dir).expect("the dump directory goes");
    listings
}

#[test]
fn separate_processes_with_one_seed_lower_the_same_code() {
    for v in DIALECTS {
        let first = code_in_a_process_of_its_own(v, 0);
        assert!(!first.is_empty(), "{v:?}: no trace was compiled");
        for run in 1..4 {
            assert!(
                code_in_a_process_of_its_own(v, run) == first,
                "{v:?}: process {run} lowered other instructions than process 0"
            );
        }
    }
}
