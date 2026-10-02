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
//!
//! `const_operand_forms_round_trip_through_luna` needs no interpreter: the
//! constant- and immediate-operand opcodes luna's compiler emits are dumped
//! under every dialect and the dump is run against the source.

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
        // luna has no time zones: its local time is UTC
        .env("TZ", "UTC")
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

/// Every constant- and immediate-operand opcode, each side of the operator,
/// with immediates PUC's parser can and cannot negate (`x - 128`,
/// `x << 128`), float immediates, metamethods that show their argument
/// order and `math.type`, and an arithmetic error naming its operand.
const CONST_OPERANDS: &str = r#"
local mtype = math.type or type
local function tag(v)
  if type(v) == "table" then return "obj" end
  if type(v) == "number" then return mtype(v) .. ":" .. tostring(v) end
  return type(v) .. ":" .. tostring(v)
end
local mt = {}
for _, ev in ipairs { "add", "sub", "mul", "div", "mod", "pow", "idiv", "band", "bor", "bxor", "shl", "shr" } do
  mt["__" .. ev] = function(a, b) return ev .. "(" .. tag(a) .. "," .. tag(b) .. ")" end
end
mt.__lt = function(a, b) print("lt(" .. tag(a) .. "," .. tag(b) .. ")") return true end
mt.__le = function(a, b) print("le(" .. tag(a) .. "," .. tag(b) .. ")") return false end
local obj = setmetatable({}, mt)
print(obj + 1, 1 + obj, obj + 128, obj + -127, obj - 1, obj - 128, obj - -127, obj - 0, 1 - obj)
print(obj * 3, 3 * obj, obj / 4, obj % 7, obj ^ 2, obj + 0.5, 0.5 + obj, obj * 1000000, obj - 1000)
-- 5.1 calls an order metamethod only for two operands of the same type
if _VERSION >= "Lua 5.2" then
  print(obj < 2, 2 < obj, obj <= 2, 2 <= obj, obj > 2, 2 > obj, obj >= 2, 2 >= obj)
  print(obj < 2.0, 2.0 < obj, obj <= -5.0, obj < -128)
end
print(obj == 1, 1 == obj, obj ~= 1, obj == 1.0, obj == "s")
local x = 7
print(x + 1, x + 128, x + -127, x - 1, x - 128, x - -127, x - 0, x * 3, x / 4, x % 7, x ^ 2)
print(x + 0.5, x * 2.5, x % 1000, x % -7, x / -3, x ^ -1, 4 / x, 7 % x, 2 ^ x)
print(x == 7, x == 8, 7 == x, x ~= 7, x == 7.0, x == 1000, x == "7", x ~= "abc")
print(x < 8, x <= 7, x > 6, x >= 7, 8 > x, 6 < x, x < -128, x > 128, x < 7.0, x <= 7.5, x >= 128, 128 <= x)
local y = -3.5
print(y + 1, y - 1, y * 2, y < 0, y <= -3, y >= -4, y == -3.5, y > -4.0, y + 0.5)
local s = "10"
print(s + 1, 1 + s, s - 1, s * 2, s < "5", s == 10, s == "10")
if _VERSION >= "Lua 5.3" then
  local f = load([[
    local obj, x = ...
    print(obj // 3, 3 // obj, obj & 12, 12 & obj, obj | 1, 1 | obj, obj ~ 255, 255 ~ obj)
    print(obj << 1, obj << 128, obj >> 1, obj << -3, obj >> -3, obj >> 128, 3 << obj, 3 >> obj)
    print(x // 3, x // -3, x & 12, 12 & x, x | 1, x ~ 255, x << 1, x << 128, x >> 1, x << -3, x >> -3)
    print(x << 63, x >> 70, x & 1000, x // 0.5, math.maxinteger + 1 == math.mininteger, 3 << x, 3 >> x)
    print(x < 7.0, x == 7.0, x ~= 7.0, x < 2^53, x & 1.0, x | 2.0)
  ]])
  f(obj, 7)
end
local function loops(n)
  local acc, c = 0, 0
  for i = 1, n do
    acc = acc + i % 7 - 1
    if i < 10 then c = c + 1 end
    if i >= 90 then c = c + 100 end
    if 50 <= i then c = c + 1000 end
    if i == 42 then c = c + 1000000 end
    if i ~= 42 then c = c - 1 end
  end
  return acc, c
end
print(loops(100))
"#;

/// Errors name the operand that is a local; the chunk name in front
/// differs between the source and its dump, the rest must not. Needs the
/// debug information a stripped dump drops.
const CONST_OPERAND_ERRORS: &str = r#"
local function err(f)
  local ok, e = pcall(f)
  return ok, (tostring(e):gsub("^[^:]*:%d+: ", ""))
end
print(err(function() local n = nil return n + 1 end))
print(err(function() local n = {} return 2 * n end))
print(err(function() local n = "abc" return n - 1 end))
print(err(function() local n = false return n < 2 end))
print(err(function() local n = false return 2 < n end))
print(err(function() local n = {} return n % 7 end))
if _VERSION >= "Lua 5.3" then
  print(err(load("local n = 2.5 return n & 1")))
  print(err(load("local n = 2.5 return n << 3")))
  print(err(load("local n = {} return 1 | n")))
end
"#;

/// Dump `source` under `version`, load the dump back and run it: the same
/// outcome as running the source, which must have printed `expect`.
fn round_trip_on_luna(
    dialect: &str,
    version: LuaVersion,
    source: &str,
    expect: &str,
    strips: &[bool],
    failed: &mut Vec<String>,
) {
    let want = run_on_luna(version, source.as_bytes()).expect("source runs");
    assert!(
        matches!(&want, Outcome::Output(s) if s.contains(expect)),
        "{dialect}: the source did not print {expect:?}: {want:?}"
    );
    for &strip in strips {
        let got = luna_dump(version, source, strip).and_then(|bytes| run_on_luna(version, &bytes));
        if got.as_ref() != Ok(&want) {
            failed.push(format!(
                "{dialect} strip={strip}:\n--- source ---\n{want:?}\n--- dump ---\n{got:?}"
            ));
        }
    }
}

#[test]
fn const_operand_forms_round_trip_through_luna() {
    let mut failed = Vec::new();
    for &(dialect, version, _, _) in DIALECTS {
        round_trip_on_luna(
            dialect,
            version,
            CONST_OPERANDS,
            "sub(obj,",
            &[false, true],
            &mut failed,
        );
        round_trip_on_luna(
            dialect,
            version,
            CONST_OPERAND_ERRORS,
            "local 'n'",
            &[false],
            &mut failed,
        );
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
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
