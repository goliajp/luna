//! `string.dump` writes the code PUC's own compiler makes for constant
//! expressions, constant operands, stores and loops: PUC's `luac -l -l`
//! lists luna's dump of a chunk exactly as it lists the chunk compiled
//! from source (instructions, constants, locals and upvalues; addresses
//! aside). The writer writes luna's registers as they are, and the frame
//! of every function, read off luna's own prototypes, is PUC's.
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

pub(crate) const DIALECTS: &[(LuaVersion, &str)] = &[
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

/// `for` loops of both kinds: their hidden control registers, the loop
/// variables after them, captured loop and body variables, nesting, and
/// generic loops of one to five variables.
const LOOPS: &[&str] = &[
    "local t = {}\nfor k, v in pairs(t) do print(k, v) end\nfor i = 1, 10 do print(i) end\n\
     for i = 1, 10, 2 do local x = i * 2; print(x) end\nfor i = 10.5, 1, -0.5 do print(i) end\n",
    "local t, fs = {}, {}\nfor i = 1, 3 do fs[i] = function() return i end end\n\
     for k, v in pairs(t) do fs[k] = function() return v end end\n\
     for k, v in pairs(t) do local w = v; fs[k] = function() return w end end\n\
     for i = 3, 1, -1 do local y = i; fs[y] = function() return y end end\nreturn fs\n",
    "local function f(t)\n  for a, b, c in ipairs(t) do\n    for i = 1, #t do\n\
           if t[i] == a then return b end\n    end\n  end\n  return nil\nend\nreturn f\n",
    "for k in next, {} do end\nfor i = 1, 2 do end\nlocal s = 0\n\
     for _, v in ipairs({1, 2, 3}) do s = s + v end\nreturn s\n",
    "local function g(...)\n  local n = 0\n  for i = 1, select('#', ...) do n = n + (select(i, ...)) end\n\
       for k, v, w, x, y in pairs({...}) do n = n + v end\n  return n\nend\nreturn g\n",
    "local t = {}\nfor i = 1, 3 do\n  for j = i, 3 do\n    for k, v in pairs(t) do\n\
           t[i + j] = k\n    end\n  end\nend\nreturn t\n",
];

/// Constant operands of the operators: before 5.4 any constant on either
/// side (or both) is an `RK` operand, from 5.4 on the immediate and `K`
/// forms take numbers and `EQK` any constant.
const OPERANDS: &[&str] = &[
    "local x, y = ...\nlocal a = '1' + x\nlocal b = x + '1'\nlocal c = 1 - x\nlocal d = 2 ^ x\n\
     local e = 5 % x\nlocal f = x < 1e300\nlocal g = 'a' <= x\nlocal h = x == nil\n\
     local i = nil == x\nlocal j = x + nil\nlocal k = true < x\nlocal l = 1 / 0\nlocal m = 0 / 0\n\
     local n = 1 < 2\nlocal o = nil == false\nlocal p = x > 'b'\nlocal q = 3 >= x\nlocal r = x ~= true\n\
     local s = 2 * 0.0\nif x == 'k' then return 1 end\nif 'k' ~= x then return 2 end\n\
     if 1.5 <= x then return 3 end\nreturn a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q, r, s\n",
    "local x, y = ...\nlocal a = x // 2.5\nlocal b = 7 // x\nlocal c = x & 1.5\nlocal d = 1 << x\n\
     local e = x >> 100000\nlocal f = '3' | x\nlocal g = x ~ 'a'\nlocal h = 1 // 0\nlocal i = 3 & 1.5\n\
     local j = 1 % 0\nlocal k = x - 0\nlocal l = -1 >> x\nlocal m = x - 128\nlocal n = x << 128\n\
     local o = 128 << x\nlocal p = x - -127\nlocal q = x >> 128\n\
     return a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q\n",
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
pub(crate) fn compare(version: LuaVersion, luac: &str, source: &str) -> Option<String> {
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

/// The frame size of each function luna compiles from `source`, read off
/// the prototypes themselves rather than through `string.dump`, against
/// the `slots` of PUC's listing (functions in the same order). `None` when
/// they agree or PUC cannot parse the chunk.
pub(crate) fn frames(version: LuaVersion, luac: &str, source: &str) -> Option<String> {
    let src = temp_path("frames.lua");
    std::fs::write(&src, source).expect("write source");
    let puc = listing(luac, &src, true);
    let _ = std::fs::remove_file(&src); // a leftover temp file is harmless
    let puc = puc?;
    let parts: Vec<&str> = puc.split(" slots").collect();
    // the count before each " slots"; the text after the last one has none
    let puc: Vec<u32> = parts[..parts.len() - 1]
        .iter()
        .filter_map(|s| s.rsplit(' ').next()?.parse().ok())
        .collect();
    let mut vm = Vm::new(version);
    let f = vm
        .load(source.as_bytes(), b"=frames")
        .expect("luna compiles it");
    let mut luna = Vec::new();
    let mut stack = vec![f.proto];
    while let Some(p) = stack.pop() {
        luna.push(u32::from(p.max_stack));
        stack.extend(p.protos.iter().rev().copied());
    }
    (puc != luna).then(|| format!("{version:?} {source:?}\nslots: PUC {puc:?}, luna {luna:?}"))
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
            .chain(LOOPS.iter().map(|s| s.to_string()))
            .chain(OPERANDS.iter().map(|s| s.to_string()))
            .chain([many_constants()]);
        failed.extend(cases.filter_map(|src| {
            compare(version, &luac, &src).or_else(|| frames(version, &luac, &src))
        }));
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
