//! `string.dump` writes the running dialect's PUC bytecode.
//!
//! Every diff_puc fixture (`tests/diff_puc/5.x/*.lua`) is compiled by luna
//! and dumped with luna's `string.dump`; then
//!
//! - `dump_runs_on_puc`: the stock PUC interpreter of that dialect loads
//!   and runs the dump, and its stdout (or, for `_err` fixtures, its error)
//!   matches PUC running the source;
//! - `dump_round_trips_through_luna`: luna loads the dump back (through
//!   its PUC translator) and runs it to the same outcome;
//! - `stripped_dump_runs_on_puc` (5.3+, where `string.dump` strips): PUC
//!   runs `string.dump(f, true)` to the outcome of PUC running the same
//!   program stripped by its own `luac -s`.
//!
//! Needs `PUC_LUA_51` … `PUC_LUA_55` (and `PUC_LUAC_5x` for the stripped
//! case); a dialect without them is skipped with a notice, or fails under
//! `LUNA_DIFF_PUC_REQUIRE_ALL=1`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: &[(&str, LuaVersion, &str, &str)] = &[
    ("5.1", LuaVersion::Lua51, "PUC_LUA_51", "PUC_LUAC_51"),
    ("5.2", LuaVersion::Lua52, "PUC_LUA_52", "PUC_LUAC_52"),
    ("5.3", LuaVersion::Lua53, "PUC_LUA_53", "PUC_LUAC_53"),
    ("5.4", LuaVersion::Lua54, "PUC_LUA_54", "PUC_LUAC_54"),
    ("5.5", LuaVersion::Lua55, "PUC_LUA_55", "PUC_LUAC_55"),
];

fn require_all() -> bool {
    std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_some()
}

fn fixtures(dialect: &str) -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/diff_puc")
        .join(dialect);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    out.retain(|p| p.extension().is_some_and(|x| x == "lua"));
    out.sort();
    out
}

#[derive(PartialEq, Debug)]
enum Outcome {
    Output(String),
    Error(bool, String),
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}

/// A leading `<chunk>:<line>: ` is dropped, its presence kept.
fn normalize_err(text: &str) -> (bool, String) {
    let t = text.trim_end();
    if let Some(colon2) = t.find(": ") {
        let head = &t[..colon2];
        if let Some(colon1) = head.rfind(':') {
            let (chunk, line) = head.split_at(colon1);
            if !chunk.is_empty() && line[1..].bytes().all(|c| c.is_ascii_digit()) {
                return (true, t[colon2 + 2..].to_string());
            }
        }
    }
    (false, t.to_string())
}

/// Run PUC's standalone interpreter on `args`, feeding `stdin`.
fn run_puc(bin: &str, args: &[&Path], stdin: &[u8]) -> Outcome {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot run `{bin}`: {e}"));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(stdin)
        .expect("write stdin");
    let out = child.wait_with_output().expect("PUC wait");
    if out.status.success() && out.stderr.is_empty() {
        return Outcome::Output(normalize(&String::from_utf8_lossy(&out.stdout)));
    }
    // `<progname>: <message>`, then a traceback
    let stderr = String::from_utf8_lossy(&out.stderr);
    let first = stderr.lines().next().unwrap_or("");
    let msg = first.split_once(": ").map_or(first, |(_, m)| m);
    let (pos, text) = normalize_err(msg);
    Outcome::Error(pos, text)
}

fn temp_file(tag: &str, bytes: &[u8]) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "luna-puc-dump-{}-{seq}-{tag}.luac",
        std::process::id()
    ));
    std::fs::write(&path, bytes).expect("write temp chunk");
    path
}

/// luna's `string.dump` of `source` compiled under the chunk name PUC's
/// `lua -` gives stdin, so positions in messages agree.
fn luna_dump(version: LuaVersion, source: &str, strip: bool) -> Result<Vec<u8>, String> {
    let mut vm = Vm::new(version);
    let f = vm
        .load(source.as_bytes(), b"=stdin")
        .map_err(|e| format!("luna does not compile it: {}", e.msg_str()))?;
    let dump = vm.eval("return string.dump").expect("string.dump")[0];
    let r = vm
        .call_value(dump, &[Value::Closure(f), Value::Bool(strip)])
        .map_err(|e| format!("string.dump failed: {}", vm.error_display(&e)))?;
    match r.first() {
        Some(Value::Str(s)) => Ok(s.as_bytes().to_vec()),
        other => Err(format!("string.dump returned {other:?}")),
    }
}

/// Load `bytes` into a default luna Vm and run it, capturing what it
/// prints the way diff_puc.rs does.
fn run_on_luna(version: LuaVersion, bytes: &[u8]) -> Result<Outcome, String> {
    let mut vm = Vm::new(version);
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
    let f = vm
        .load(bytes, b"=dump")
        .map_err(|e| format!("luna refuses its own dump: {}", e.msg_str()))?;
    if let Err(e) = vm.call_value(Value::Closure(f), &[]) {
        let (pos, text) = normalize_err(&vm.error_display(&e));
        return Ok(Outcome::Error(pos, text));
    }
    match vm
        .eval("return _G.__luna_diff_puc_buf")
        .expect("buffer")
        .first()
    {
        Some(Value::Str(s)) => Ok(Outcome::Output(normalize(&String::from_utf8_lossy(
            s.as_bytes(),
        )))),
        other => Err(format!("expected the capture buffer, got {other:?}")),
    }
}

/// Run `check` on every fixture of every dialect whose interpreter is
/// available; report the per-dialect pass counts and fail on any miss.
fn each_fixture(
    name: &str,
    min: LuaVersion,
    check: impl Fn(&Path, LuaVersion, &str, &str, &str) -> Result<(), String>,
) {
    let mut report = Vec::new();
    let mut failed = Vec::new();
    for &(dialect, version, lua_key, luac_key) in DIALECTS.iter().filter(|d| d.1 >= min) {
        let (Ok(lua), luac) = (std::env::var(lua_key), std::env::var(luac_key)) else {
            assert!(!require_all(), "[{name}] {lua_key} must be set");
            eprintln!("[{name}] {dialect}: SKIPPED — {lua_key} not set");
            continue;
        };
        let luac = luac.unwrap_or_default();
        let all = fixtures(dialect);
        let mut pass = 0;
        for f in &all {
            let source = std::fs::read_to_string(f).expect("read fixture");
            match check(f, version, &lua, &luac, &source) {
                Ok(()) => pass += 1,
                Err(e) => failed.push(format!("{}: {e}", f.display())),
            }
        }
        report.push(format!("{dialect} {pass}/{}", all.len()));
    }
    eprintln!("[{name}] passed: {}", report.join(", "));
    assert!(
        failed.is_empty(),
        "[{name}] {} fixtures fail:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn dump_runs_on_puc() {
    each_fixture(
        "dump_runs_on_puc",
        LuaVersion::Lua51,
        |_, version, lua, _, source| {
            let want = run_puc(lua, &[Path::new("-")], source.as_bytes());
            let bytes = luna_dump(version, source, false)?;
            let chunk = temp_file("run", &bytes);
            let got = run_puc(lua, &[&chunk], b"");
            let _ = std::fs::remove_file(&chunk); // a leftover temp file is harmless
            if got != want {
                return Err(format!(
                    "\n--- PUC source ---\n{want:?}\n--- PUC dump ---\n{got:?}"
                ));
            }
            Ok(())
        },
    );
}

#[test]
fn dump_round_trips_through_luna() {
    each_fixture(
        "dump_round_trips_through_luna",
        LuaVersion::Lua51,
        |_, version, lua, _, source| {
            let want = run_puc(lua, &[Path::new("-")], source.as_bytes());
            let bytes = luna_dump(version, source, false)?;
            let got = std::panic::catch_unwind(|| run_on_luna(version, &bytes))
                .map_err(|_| "luna panicked".to_string())??;
            if got != want {
                return Err(format!(
                    "\n--- PUC source ---\n{want:?}\n--- luna ---\n{got:?}"
                ));
            }
            Ok(())
        },
    );
}

#[test]
fn stripped_dump_runs_on_puc() {
    each_fixture(
        "stripped_dump_runs_on_puc",
        LuaVersion::Lua53,
        |f, version, lua, luac, source| {
            if luac.is_empty() {
                return Err("no luac for this dialect".to_string());
            }
            let stripped = temp_file("luac", b"");
            let st = Command::new(luac)
                .arg("-s")
                .arg("-o")
                .arg(&stripped)
                .arg(f)
                .status()
                .map_err(|e| format!("cannot run luac: {e}"))?;
            assert!(st.success(), "luac -s failed on {}", f.display());
            let want = run_puc(lua, &[&stripped], b"");
            let _ = std::fs::remove_file(&stripped); // a leftover temp file is harmless
            let bytes = luna_dump(version, source, true)?;
            let chunk = temp_file("strip", &bytes);
            let got = run_puc(lua, &[&chunk], b"");
            let _ = std::fs::remove_file(&chunk); // a leftover temp file is harmless
            if got != want {
                return Err(format!(
                    "\n--- PUC luac -s ---\n{want:?}\n--- PUC dump ---\n{got:?}"
                ));
            }
            Ok(())
        },
    );
}
