//! `string.dump` writes the code PUC's own compiler makes for constant
//! expressions: PUC's `luac -l -l` lists luna's dump of a chunk exactly as
//! it lists the chunk compiled from source (instructions, constants,
//! locals and upvalues; addresses aside).
//!
//! Each case is a table constructor holding a constant expression, which
//! PUC's parser folds to a number (`2^53`, `7 // 2`, `~5.0`) or leaves to
//! run time (`-0.0`, `~5.5`). A case a dialect cannot parse is skipped
//! for it. Needs `PUC_LUAC_51` … `PUC_LUAC_55`; a dialect without one is
//! skipped with a notice, or fails under `LUNA_DIFF_PUC_REQUIRE_ALL=1`.

use std::path::Path;
use std::process::Command;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "PUC_LUAC_51"),
    (LuaVersion::Lua52, "PUC_LUAC_52"),
    (LuaVersion::Lua53, "PUC_LUAC_53"),
    (LuaVersion::Lua54, "PUC_LUAC_54"),
    (LuaVersion::Lua55, "PUC_LUAC_55"),
];

/// The constant expressions, separated by ", ".
const EXPRS: &str = "2^53, 2^2, 2^0.5, 3^-1, 0^0, 2^1024, -(2^53), 7//2, 7.0//2, -7//2, 3//1.0, \
    -(1//1), 7%3, -7%3, 7%-3, 7.5%2, -7.5%2, 0%5, -(7%3), 3&5, 3|5, 3~5, 1<<4, 1<<64, 1<<-1, \
    256>>4, -1>>1, 3.0&5, 2^53|0, ~5, ~5.0, ~5.5, -0.0, -(0.0), - -0.0, -0, 1.5+1.5, math.pi*2";

/// A constructor mixing registers, constants and a folded power.
const TABLE: &str = "local a, b = ...\nlocal t = {a, b, 'k', 1.5, 2^53, true}\nreturn t\n";

fn temp_path(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "luna-puc-listing-{}-{seq}-{tag}",
        std::process::id()
    ))
}

/// `luac -l -l` of `file`, without addresses; `None` when luac rejects it.
fn listing(luac: &str, file: &Path, parse_only: bool) -> Option<String> {
    let mut cmd = Command::new(luac);
    cmd.args(["-l", "-l"]);
    if parse_only {
        cmd.arg("-p");
    }
    let out = cmd
        .arg(file)
        .output()
        .unwrap_or_else(|e| panic!("cannot run `{luac}`: {e}"));
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(
        text.split_whitespace()
            .filter(|w| !w.starts_with("0x"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// luna's `string.dump` of the chunk in `src`, compiled under the name
/// luac gives it.
fn luna_dump(version: LuaVersion, src: &Path) -> Vec<u8> {
    let mut vm = Vm::new(version);
    let source = std::fs::read(src).expect("read source");
    let name = format!("@{}", src.display());
    let f = vm.load(&source, name.as_bytes()).expect("luna compiles it");
    let dump = vm.eval("return string.dump").expect("string.dump")[0];
    match vm
        .call_value(dump, &[Value::Closure(f)])
        .expect("dump")
        .first()
    {
        Some(Value::Str(s)) => s.as_bytes().to_vec(),
        other => panic!("string.dump returned {other:?}"),
    }
}

/// `None` when the listings agree or PUC cannot parse `source`.
fn compare(version: LuaVersion, luac: &str, source: &str) -> Option<String> {
    let src = temp_path("src.lua");
    std::fs::write(&src, source).expect("write source");
    let want = listing(luac, &src, true);
    let got = want.as_ref().map(|_| {
        let bin = temp_path("dump.luac");
        std::fs::write(&bin, luna_dump(version, &src)).expect("write dump");
        let got = listing(luac, &bin, false).expect("luac lists luna's dump");
        let _ = std::fs::remove_file(&bin); // a leftover temp file is harmless
        got
    });
    let _ = std::fs::remove_file(&src); // a leftover temp file is harmless
    match (want, got) {
        (Some(w), Some(g)) if w != g => Some(format!(
            "{version:?} {source:?}\n--- PUC ---\n{w}\n--- luna ---\n{g}"
        )),
        _ => None,
    }
}

#[test]
fn constant_expressions_compile_as_puc_compiles_them() {
    let mut failed = Vec::new();
    for &(version, var) in DIALECTS {
        let Some(luac) = std::env::var(var).ok().filter(|s| !s.is_empty()) else {
            assert!(
                std::env::var_os("LUNA_DIFF_PUC_REQUIRE_ALL").is_none(),
                "{var} is not set"
            );
            eprintln!("skipping {version:?}: {var} is not set");
            continue;
        };
        let cases = EXPRS
            .split(", ")
            .map(|e| format!("local x = {{{e}, 1}}\nreturn x\n"))
            .chain([TABLE.to_string()]);
        failed.extend(cases.filter_map(|src| compare(version, &luac, &src)));
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
