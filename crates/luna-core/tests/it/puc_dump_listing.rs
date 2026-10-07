//! `string.dump` writes the code PUC's own compiler makes for constant
//! expressions: PUC's `luac -l -l` lists luna's dump of a chunk exactly as
//! it lists the chunk compiled from source (instructions, constants,
//! locals and upvalues; addresses aside).
//!
//! Each case is a table constructor holding a constant expression, which
//! PUC's parser folds to a number (`2^53`, `7 // 2`, `~5.0`) or leaves to
//! run time (`-0.0`, `7.5 // 0`, `5 % math.huge`). A case a dialect cannot parse is skipped
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
const EXPRS: &str = "2^53, 2^2, 2^0.5, (-2)^0.5, 3^-1, 0^0, 7//2, 7.0//2, -7//2, 7//0, 7.5//0, \
    1//-0.0, 7%3, -7%3, 7%-3, 7.5%2, -7.5%2, 5%0, 5.0%0, 5%math.huge, 0%5, 0.0%5, -0.0%5, 3&5, \
    3|5, 3~5, 1<<4, 1<<64, 1<<-1, 256>>4, -1>>1, 3.0&5, 3.5&5, 2^53|0, \"3\"&5, ~5, ~5.0, ~5.5, \
    -0.0, -(0.0), - -0.0, -0, -(2^53), -(1//1), 0.0*-1, 1/0, -1/0, 0/0, 2^1024, 1e308*10, \
    5.0//0.0, -(7%3), 1.5+1.5, 3-3.0, 2*0.0, 3//1.0, math.pi*2, 7-(1 ..\"9\"), \
    0xffffffff>>(1 ..\"9\"), 10|(1 ..\"9\"), 2^(1 ..\"9\")";

/// Chunks whose constants PUC's code generator shares or keeps apart by
/// dialect: an integer and the float of its value, -0.0 and 0.0, nan
/// (folded by 5.2 only), short and long strings, a string whose eight bytes
/// are those of 0.0, and floats past 2^53 whose keys meet integers.
const DEDUP: &[&str] = &[
    "local a, b, c, d = ...\nreturn a + 100000, b + 100000.0, c * 100000, d * 100000.0, a - 5, b - 5.0\n",
    "local a, b, c = ...\nreturn a * 0.0, b * -0.0, c * 0.0, a + 0, b - 0.0, c / -0.0\n",
    "local a, b = ...\nreturn a + (-2)^0.5, b + (-2)^0.5, a * 0/0\n",
    "local a, b = ...\nreturn a .. 'k', b .. 'k', a .. 'kk', \
     a .. 'a long string, past the forty bytes PUC keeps short', \
     b .. 'a long string, past the forty bytes PUC keeps short'\n",
    "local a, b = ...\nreturn a .. '\\0\\0\\0\\0\\0\\0\\0\\0', b * 0.0, a * 0.0\n",
    "local a, b, c = ...\nreturn a + 2^53, b + 9007199254740994, c + 2^53, a + 2^60, b + 2^60\n",
    "local a, b = ...\nreturn a + 1e300, b + 1e300, a + 0.5, b + 0.5, a + 5, b + 5\n",
    "local a, b, c, d = 100000, 100000.0, 5, 5.0\nreturn a, b, c, d, 100000.0, 100000\n",
    "local a, b, c, d = 0.0, -0.0, 0, -0\nreturn a, b, c, d, -0.0, 0.0\n",
    "local a, b = 0/0, -(0/0)\nreturn a, b, (-2)^0.5, (-2)^0.5\n",
    "local x = ...\nreturn x == 0.0, x == -0.0, x == 0, x < 1e300, x < 1e300\n",
    "local s = 'a long string, past the forty bytes PUC keeps short'\n\
     return s, 'a long string, past the forty bytes PUC keeps short', 'k', 'k'\n",
    "local t = {}\nt[1], t[1.0], t[2^53], t[2^53 + 1.0], t['1'] = 1, 2, 3, 4, 5\nreturn t\n",
    "local a = 9007199254740993\nreturn a, 9007199254740992.0, 2^63, -2^63, 1e15, 1e15 + 0.5\n",
];

/// Assignments: fields, globals, indexings, upvalues, several targets at
/// once, values that are constants, and function statements.
const STORES: &[&str] = &[
    "local t, u = {}, {}\nt.x, t.y, u[1], u[2] = 1, 'k', true, nil\nx, y = t, 2.5\nreturn t, u\n",
    "local t, n = {}, 0\nlocal function f(v)\n  t.a = v\n  t[v] = 1\n  t[1] = false\n  \
     n = n + 1\n  t = nil\n  g = 'k'\nend\nreturn f\n",
    "local a, i, j = {}, 1, 2\ni, a[i], a, j, a[j], a[i+j] = j, i, i, nil, j, i\nreturn a\n",
    "local a = {}\nlocal function f()\n  a.x, a = 1, 2\n  a, a.y = 3, 4\nend\nreturn f\n",
    "local t = {}\nlocal a, b\na, t.x, b = 1\nt.y, a = 1, 2, 3\na, b = f()\nt[a], t.z = g()\n\
     a, b = b, a\nreturn t\n",
    "local t = {a = {}}\nlocal f\nfunction f() end\nfunction t.a.b() end\nfunction t.a:c() end\n\
     function g() end\nfunction h.i.j() end\nreturn t\n",
    "local t, k = {}, ...\nt[1.5] = k\nt[-1] = 2\nt[300] = 1e300\nt[true] = 'v'\nt[k] = 0.5\n\
     t['a long string, past the forty bytes PUC keeps short'] = 1\n\
     local v = t[1.5] or t[300] or t[-1] or t[true] or t[k]\nreturn v\n",
    "local a = ...\nreturn {x = 1, [2] = 'k', [a] = true, [1.5] = a, ['y'] = nil, z = a + 1, a}\n",
    "local f, g = nil, {}\nf = function() return g end\ng.h = function() end\nreturn f\n",
    "local t = {}\nreturn function(k) t.x = k; t[1] = k; return t.x, t[1], t[k], t[2.5] end\n",
    "local a, b = ...\na = b + 1\nb = a.x\na.y = -b\nreturn a, b\n",
    "global x, y = 1, 2\nglobal function gf() end\nreturn x\n",
    "local t = {}\nreturn function(k)\n  t[k], k = 1, 2\n  t[true] = t[k]\n  t[-1], t[2^53] = t[0.5], t['k']\n\
     t[k + 1] = t[k] or t[1] or t[2]\nend\n",
];

/// A chunk with more constants than an `RK` operand reaches, then stores
/// whose values and keys are constants.
fn many_constants() -> String {
    let mut s = String::from("local t = {");
    for i in 0..300 {
        s.push_str(&format!("{i}.25, "));
    }
    s.push_str(
        "}\nt.a = 1.75\nt[2.5] = true\nt[7] = 'k'\ng = 3.5\nreturn t[2.5], t[300.5], g, h\n",
    );
    s
}

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
    let out = match vm.call_value(dump, &[Value::Closure(f)]) {
        Ok(out) => out,
        Err(e) => panic!(
            "{version:?} dump of {:?}: {}",
            String::from_utf8_lossy(&source),
            vm.error_text(&e)
        ),
    };
    match out.first() {
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
            .chain([TABLE.to_string()])
            .chain(DEDUP.iter().map(|s| s.to_string()))
            .chain(STORES.iter().map(|s| s.to_string()))
            .chain([many_constants()]);
        failed.extend(cases.filter_map(|src| compare(version, &luac, &src)));
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
