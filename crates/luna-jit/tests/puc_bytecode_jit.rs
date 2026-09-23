//! The diff_puc corpus as PUC bytecode, run with the Cranelift JIT.
//!
//! luna-core's `diff_puc_bytecode` pins what a translated chunk does on
//! luna's interpreter against PUC. This runs the same chunks — every
//! fixture of `luna-core/tests/diff_puc/5.x`, compiled by that dialect's
//! `luac` (`PUC_LUAC_5X`) — on a JIT-equipped Vm and requires the same
//! output, or the same error, as the interpreter gives, unless luna's own
//! compile of the program diverges under the JIT the same way: that is a
//! JIT defect, reported but outside what this test pins. The `*_hot_loops_*`
//! fixtures make the JIT compile translated code; a dialect where it never
//! engaged on them fails the test rather than passing on the interpreter
//! alone.
//!
//! `LUNA_DIFF_PUC_REQUIRE_ALL=1` makes a missing `luac` a failure, as in
//! luna-core's harness.

use std::path::{Path, PathBuf};
use std::process::Command;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: &[(&str, LuaVersion, &str)] = &[
    ("5.1", LuaVersion::Lua51, "PUC_LUAC_51"),
    ("5.2", LuaVersion::Lua52, "PUC_LUAC_52"),
    ("5.3", LuaVersion::Lua53, "PUC_LUAC_53"),
    ("5.4", LuaVersion::Lua54, "PUC_LUAC_54"),
    ("5.5", LuaVersion::Lua55, "PUC_LUAC_55"),
];

fn fixtures(dialect: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../luna-core/tests/diff_puc")
        .join(dialect);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    out.sort();
    out
}

fn compile(luac: &str, dialect: &str, path: &Path) -> Vec<u8> {
    let out = std::env::temp_dir().join(format!(
        "luna-puc-bytecode-jit-{}-{dialect}-{}.luac",
        std::process::id(),
        path.file_stem()
            .and_then(|s| s.to_str())
            .expect("file name")
    ));
    let st = Command::new(luac)
        .arg("-o")
        .arg(&out)
        .arg(path)
        .status()
        .unwrap_or_else(|e| panic!("cannot run luac `{luac}`: {e}"));
    assert!(st.success(), "luac failed on {}", path.display());
    let bytes = std::fs::read(&out).expect("read luac output");
    let _ = std::fs::remove_file(&out); // temp file; nothing depends on its removal
    bytes
}

/// Run `bytes` with `print` / `io.write` captured: the output, or the error
/// as the standalone interpreter would show it.
fn run(vm: &mut Vm, path: &Path, bytes: &[u8]) -> Result<String, String> {
    vm.set_puc_bytecode_loading(true);
    vm.eval(
        r#"
_G.__out = ""
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out = _G.__out .. table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out = _G.__out .. table.concat(t)
end
"#,
    )
    .expect("capture preamble");
    let f = vm
        .load(bytes, b"=luac")
        .unwrap_or_else(|e| panic!("{}: {}", path.display(), String::from_utf8_lossy(&e.msg)));
    if let Err(e) = vm.call_value(Value::Closure(f), &[]) {
        return Err(vm.error_display(&e));
    }
    match vm.eval("return _G.__out").expect("buffer").first() {
        Some(Value::Str(s)) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        other => panic!("expected the capture buffer, got {other:?}"),
    }
}

#[test]
fn puc_bytecode_runs_the_same_with_the_jit() {
    let require = std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_some();
    let mut failed = Vec::new();
    let mut jit_only = Vec::new();
    for &(dialect, version, key) in DIALECTS {
        let Ok(luac) = std::env::var(key) else {
            assert!(!require, "{key} must be set");
            eprintln!("[puc_bytecode_jit] {dialect}: SKIPPED — {key} not set");
            continue;
        };
        let mut jit_activity = 0u64;
        for f in &fixtures(dialect) {
            let bytes = compile(&luac, dialect, f);
            let interp = run(&mut Vm::new(version), f, &bytes);
            let mut vm = luna_jit::new_with_jit(version);
            let jit = run(&mut vm, f, &bytes);
            // Only these fixtures count: others may JIT source they load
            // with `load`, which says nothing about translated code.
            if f.to_string_lossy().contains("_hot_loops_") {
                jit_activity +=
                    vm.trace_dispatched_count() + luna_jit::jit::cache_entry_count(&vm) as u64;
            }
            if jit == interp {
                continue;
            }
            // The same program compiled by luna itself tells a JIT defect
            // (it diverges there too) from a translation one (it does not).
            let source = std::fs::read(f).expect("read fixture");
            let src_interp = run(&mut Vm::new(version), f, &source);
            let src_jit = run(&mut luna_jit::new_with_jit(version), f, &source);
            let line = format!(
                "{}:\n  interpreter: {interp:?}\n  jit:         {jit:?}",
                f.display()
            );
            if src_jit == src_interp {
                failed.push(line);
            } else {
                jit_only.push(line);
            }
        }
        assert!(
            jit_activity > 0,
            "[puc_bytecode_jit] {dialect}: the JIT never engaged, so nothing was compared"
        );
    }
    if !jit_only.is_empty() {
        eprintln!(
            "[puc_bytecode_jit] the JIT diverges from the interpreter on luna's own \
             compile of these programs as well — a JIT defect, not a translation one:\n{}",
            jit_only.join("\n")
        );
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
