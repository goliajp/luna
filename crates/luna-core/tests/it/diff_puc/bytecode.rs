//! Fixtures compiled by PUC `luac`, loaded and run by both engines.

use super::*;

const LUAC_ENV: &[(&str, &str)] = &[
    ("5.1", "PUC_LUAC_51"),
    ("5.2", "PUC_LUAC_52"),
    ("5.3", "PUC_LUAC_53"),
    ("5.4", "PUC_LUAC_54"),
    ("5.5", "PUC_LUAC_55"),
];

fn is_err_fixture(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.ends_with("_err"))
}

/// Compile `path` with PUC's `luac` into the system temp dir, under a name
/// no other test thread of this process uses.
fn compile_with_luac(luac: &str, dialect: &str, path: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!(
        "luna-diff-puc-{}-{seq}-{dialect}-{}.luac",
        std::process::id(),
        path.file_stem()
            .and_then(|s| s.to_str())
            .expect("fixture file name")
    ));
    let st = Command::new(luac)
        .arg("-o")
        .arg(&out)
        .arg(path)
        .status()
        .unwrap_or_else(|e| panic!("[diff_puc] cannot run luac `{luac}`: {e}"));
    assert!(st.success(), "[diff_puc] luac failed on {}", path.display());
    out
}

/// What running a chunk produced: its captured output, or its top-level
/// error normalized by `normalize_err`.
#[derive(PartialEq, Debug)]
enum Outcome {
    Output(String),
    Error(bool, String),
}

/// Run a precompiled chunk on PUC.
fn run_luac_on_puc(bin: &str, luac_file: &Path) -> Outcome {
    let out = Command::new(bin)
        .arg(luac_file)
        .env("TZ", "UTC")
        .output()
        .unwrap_or_else(|e| panic!("[diff_puc] cannot run `{bin}`: {e}"));
    if out.status.success() && out.stderr.is_empty() {
        return Outcome::Output(normalize(&String::from_utf8_lossy(&out.stdout)));
    }
    // The standalone prints `<progname>: <message>` then a traceback.
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().next().unwrap_or("");
    let msg = first.split_once(": ").map_or(first, |(_, m)| m);
    let (pos, text) = normalize_err(msg);
    Outcome::Error(pos, text)
}

/// Load PUC bytecode into luna and run it with stdout captured the way
/// `run_on_luna` does. A load error is a failure of the translator, not an
/// outcome to compare.
fn run_luac_on_luna(vm: &mut Vm, path: &Path, bytes: &[u8]) -> Result<Outcome, String> {
    vm.set_puc_bytecode_loading(true);
    vm.eval(
        r#"
_G.__luna_diff_puc_buf = ""
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__luna_diff_puc_buf = _G.__luna_diff_puc_buf .. table.concat(t)
end
"#,
    )
    .expect("capture preamble");
    let f = vm.load(bytes, b"=luac").map_err(|e| {
        format!(
            "luna rejects PUC bytecode of {}: {}",
            path.display(),
            String::from_utf8_lossy(&e.msg)
        )
    })?;
    if let Err(e) = vm.call_value(luna_core::runtime::Value::Closure(f), &[]) {
        let (pos, text) = normalize_err(&vm.error_display(&e));
        return Ok(Outcome::Error(pos, text));
    }
    match vm
        .eval("return _G.__luna_diff_puc_buf")
        .expect("buffer")
        .first()
    {
        Some(luna_core::runtime::Value::Str(s)) => Ok(Outcome::Output(normalize(
            &String::from_utf8_lossy(s.as_bytes()),
        ))),
        other => Err(format!("expected the capture buffer, got {other:?}")),
    }
}

/// Every fixture of every dialect, compiled with that dialect's stock
/// `luac`, run as bytecode on PUC and on luna: the corpus pins what each
/// program prints (stdout mode) or how it fails (`_err` mode), so it
/// doubles as an end-to-end test of the PUC chunk translators.
#[test]
fn diff_puc_bytecode() {
    let mut total = 0usize;
    let mut report = Vec::new();
    for &(dialect, version, env_key) in DIALECTS {
        let luac_key = LUAC_ENV
            .iter()
            .find(|(d, _)| *d == dialect)
            .map(|(_, k)| *k)
            .expect("every dialect has a luac env key");
        let (Ok(luac), Ok(bin)) = (std::env::var(luac_key), std::env::var(env_key)) else {
            assert!(
                !require_all(),
                "[diff_puc] bytecode {dialect}: {luac_key} and {env_key} must be set"
            );
            eprintln!("[diff_puc] bytecode {dialect}: SKIPPED — {luac_key} or {env_key} not set");
            continue;
        };
        let fixtures = list_fixtures(dialect);
        eprintln!(
            "[diff_puc] bytecode {dialect}: running {} fixtures via {luac}",
            fixtures.len()
        );
        let mut failed = Vec::new();
        for f in &fixtures {
            let luac_file = compile_with_luac(&luac, dialect, f);
            let bytes = std::fs::read(&luac_file).expect("read luac output");
            let puc = run_luac_on_puc(&bin, &luac_file);
            let _ = std::fs::remove_file(&luac_file); // temp file; nothing depends on its removal
            match (&puc, is_err_fixture(f)) {
                (Outcome::Output(_), false) | (Outcome::Error(..), true) => {}
                _ => panic!(
                    "[diff_puc] {} (as bytecode) on PUC itself: {puc:?} — fix the fixture",
                    f.display()
                ),
            }
            let luna =
                std::panic::catch_unwind(|| run_luac_on_luna(&mut Vm::new(version), f, &bytes));
            match luna {
                Ok(Ok(out)) if out == puc => {}
                Ok(Ok(out)) => failed.push(format!(
                    "{}: differs\n--- PUC ---\n{puc:?}\n--- luna ---\n{out:?}",
                    f.display()
                )),
                Ok(Err(e)) => failed.push(format!("{}: {e}", f.display())),
                Err(e) => failed.push(format!(
                    "{}: luna panicked: {}",
                    f.display(),
                    e.downcast_ref::<String>().cloned().unwrap_or_default()
                )),
            }
        }
        total += fixtures.len();
        if !failed.is_empty() {
            report.push(format!(
                "[diff_puc] bytecode {dialect}: {} of {} fixtures fail:\n{}",
                failed.len(),
                fixtures.len(),
                failed.join("\n")
            ));
        }
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
    if require_all() {
        assert!(total > 0, "[diff_puc] bytecode: no fixture ran");
    }
}

/// A chunk from any dialect's `luac` loads into a Vm of every other dialect
/// and runs without a panic. It runs under the host dialect's library and
/// number semantics, so what it prints is not compared — the translation
/// itself must still hold, whichever dialect hosts it.
#[test]
fn puc_bytecode_loads_in_every_dialect() {
    let mut failed = Vec::new();
    let mut ran = 0usize;
    for &(dialect, _, _) in DIALECTS {
        let luac_key = LUAC_ENV
            .iter()
            .find(|(d, _)| *d == dialect)
            .map(|(_, k)| *k)
            .expect("every dialect has a luac env key");
        let Ok(luac) = std::env::var(luac_key) else {
            assert!(!require_all(), "[diff_puc] {luac_key} must be set");
            eprintln!("[diff_puc] cross-dialect {dialect}: SKIPPED — {luac_key} not set");
            continue;
        };
        for f in &list_fixtures(dialect) {
            let luac_file = compile_with_luac(&luac, dialect, f);
            let bytes = std::fs::read(&luac_file).expect("read luac output");
            let _ = std::fs::remove_file(&luac_file); // temp file; nothing depends on its removal
            for &(host, host_version, _) in DIALECTS.iter().filter(|d| d.0 != dialect) {
                let run = std::panic::catch_unwind(|| {
                    let mut vm = Vm::new(host_version);
                    // A 5.4 overflow-guarded `for` spins forever under 5.3
                    // loop semantics, as it would on PUC 5.3; the budget turns
                    // that into an error, which is an outcome like any other.
                    vm.set_instr_budget(Some(50_000_000));
                    run_luac_on_luna(&mut vm, f, &bytes)
                });
                match run {
                    Ok(Ok(_)) => ran += 1,
                    Ok(Err(e)) => failed.push(format!("{host} Vm: {e}")),
                    Err(e) => failed.push(format!(
                        "{} in a {host} Vm: luna panicked: {}",
                        f.display(),
                        e.downcast_ref::<String>().cloned().unwrap_or_default()
                    )),
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    if require_all() {
        assert!(ran > 0, "[diff_puc] cross-dialect: nothing ran");
    }
}
