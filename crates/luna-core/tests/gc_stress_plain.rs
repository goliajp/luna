//! v2.13 Track WUC Prong B — plain (native-heap) gc.lua stress.
//!
//! Same loop as `gc_stress_poison.rs` but WITHOUT the allocator
//! override: on Windows this exercises the real Heap Manager
//! freed-memory behavior that UAF-C manifests under
//! (`.dev/known-bugs/windows-gc-weak-table-uaf-c.md`). Driven by
//! `.github/workflows/uafc-windows-stress.yml` on windows-latest
//! to measure the repro rate; also runnable anywhere for a
//! baseline.
//!
//! Run:
//!     LUNA_GC_STRESS_N=50 cargo test --release -p luna-core \
//!         --test gc_stress_plain -- --ignored --nocapture

use std::path::Path;
use std::sync::mpsc;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn run_one(file: &str) -> Result<(), String> {
    let raw = std::fs::read(file).map_err(|e| format!("read {file}: {e}"))?;
    let stripped = luna_core::frontend::lexer::Lexer::strip_shebang_bom(&raw).to_vec();
    let chunkname = format!("@{file}");
    let label = file.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = Vm::new(LuaVersion::Lua55);
            vm.set_global("_U", Value::Bool(true)).unwrap();
            let r: Result<(), String> = match vm.load(&stripped, chunkname.as_bytes()) {
                Ok(cl) => match vm.call_value(Value::Closure(cl), &[]) {
                    Ok(_) => Ok(()),
                    Err(e) => Err(format!("runtime: {:.300}", vm.error_text(&e))),
                },
                Err(e) => Err(format!("compile: {e}")),
            };
            let _ = tx.send(r);
        })
        .map_err(|e| format!("spawn {label}: {e}"))?
        .join()
        .map_err(|_| format!("{file}: worker thread panicked (UAF-C candidate!)"))?;
    rx.recv().map_err(|e| format!("recv: {e}"))?
}

#[test]
#[ignore = "UAF-C diagnostic stress (slow); run with --ignored"]
fn gc_lua_family_plain_stress() {
    let n: usize = std::env::var("LUNA_GC_STRESS_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    // LUNA_GC_STRESS_SUITE overrides the suite dir (bisection runs a
    // copied suite with a truncated gc.lua while keeping the relative
    // chunkname "gc.lua" — manifestation is allocation-order sensitive).
    let suite = match std::env::var("LUNA_GC_STRESS_SUITE") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/official/lua-5.5.0-tests"),
    };
    std::env::set_current_dir(&suite).expect("suite dir");
    // Minimization hook: LUNA_GC_STRESS_FILE=<abs path> runs just
    // that chunk (cwd stays the suite dir so require still works).
    let files: Vec<String> = match std::env::var("LUNA_GC_STRESS_FILE") {
        Ok(f) => vec![f],
        Err(_) => ["gc.lua", "gengc.lua", "tracegc.lua"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };
    for iter in 1..=n {
        for file in &files {
            eprintln!("[gc-stress] iter {iter}/{n} {file}");
            if let Err(e) = run_one(file) {
                panic!("[gc-stress] iter {iter} {file} FAILED: {e}");
            }
        }
    }
    eprintln!("[gc-stress] {n} iterations clean");
}
