//! Opening the standard libraries on a state that has them already, as a
//! host calls `luaL_openlibs` twice: 5.1 registers new functions into the
//! library tables it finds, 5.2 makes every library anew, 5.3 on only
//! points the globals back at the libraries `package.loaded` holds.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn text(vm: &mut Vm, src: &str) -> String {
    let vals = vm
        .eval(src)
        .unwrap_or_else(|e| panic!("{}", vm.error_text(&e)));
    vals.iter()
        .map(|v| match v {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) if f.fract() == 0.0 => format!("{f:.0}"),
            Value::Nil => "nil".to_string(),
            v => format!("{v:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `setup`, a second `open_all_libs`, then `check`'s results.
fn reopened(v: LuaVersion, setup: &str, check: &str) -> String {
    let mut vm = Vm::new(v);
    vm.eval(setup).expect("setup");
    vm.open_all_libs().expect("reopen");
    text(&mut vm, check)
}

#[test]
fn library_tables_are_kept_or_made_anew_as_each_dialect_does() {
    let setup = r#"R = {s = string, m = math, p = package, l = package.loaded,
        smt = getmetatable(""), fmt = getmetatable(io.stdin), si = io.stdin,
        len = string.len}"#;
    let check = r#"return string == R.s, math == R.m, package == R.p,
        package.loaded == R.l, getmetatable("") == R.smt,
        getmetatable(io.stdin) == R.fmt, io.stdin == R.si, string.len == R.len"#;
    // PUC's own answers, from a host that calls `luaL_openlibs` twice
    let want = [
        "true true true true false true false false",
        "false false false true false true false true",
        "true true true true true true true true",
        "true true true true true true true true",
        "true true true true true true true true",
    ];
    for (v, w) in ALL.into_iter().zip(want) {
        assert_eq!(reopened(v, setup, check), w, "{v:?}");
    }
}

#[test]
fn fields_a_program_added_stay_unless_the_library_is_made_anew() {
    let want = ["1 2", "nil nil", "1 2", "1 2", "1 2"];
    for (v, w) in ALL.into_iter().zip(want) {
        let got = reopened(
            v,
            "string.foo = 1; package.foo = 2",
            "return string.foo, package.foo",
        );
        assert_eq!(got, w, "{v:?}");
    }
}

/// A Vm whose libraries opened as `lua.c` opens them: with the collector
/// stopped (5.3's leaves it running).
fn lua_c_opened(v: LuaVersion) -> Vm {
    let mut vm = Vm::new_minimal(v);
    let stop = v != LuaVersion::Lua53;
    if stop {
        vm.host_gc(0, [0; 3]).expect("stop");
    }
    vm.open_all_libs().expect("open");
    if stop {
        vm.host_gc(1, [0; 3]).expect("restart");
    }
    vm
}

/// What a host that opened the libraries as `lua.c` does sees around a
/// second `open_all_libs` (with `reopen`): the collector stopped, `setup`
/// run as `luaL_dostring` runs it, then the bytes the reopening adds, and
/// the count after a full collection less the count before the reopening.
fn counts_around(v: LuaVersion, setup: &str, reopen: bool) -> (i64, i64) {
    const LUA_GCSTOP: i32 = 0;
    const LUA_GCCOLLECT: i32 = 2;
    let mut vm = lua_c_opened(v);
    vm.host_gc(LUA_GCSTOP, [0; 3]).expect("stop");
    let f = vm.load(setup.as_bytes(), setup.as_bytes()).expect("setup");
    vm.call_value(Value::Closure(f), &[]).expect("setup");
    let before = vm.heap.bytes() as i64;
    if reopen {
        vm.open_all_libs().expect("reopen");
    }
    let after = vm.heap.bytes() as i64;
    vm.host_gc(LUA_GCCOLLECT, [0; 3]).expect("collect");
    (after - before, vm.heap.bytes() as i64 - before)
}

/// The bytes a second `open_all_libs` adds, and what of them a full
/// collection after it leaves, beyond what it leaves without the reopening.
fn reopen_counts(v: LuaVersion, setup: &str) -> (i64, i64) {
    let (added, left) = counts_around(v, setup, true);
    (added, left - counts_around(v, setup, false).1)
}

// PUC's answers for a 64-bit build: the bytes the second `luaL_openlibs`
// adds, and what a collection after it leaves of them
#[test]
#[cfg(target_pointer_width = "64")]
fn reopening_counts_what_puc_counts() {
    let got: Vec<_> = ALL.iter().map(|&v| reopen_counts(v, "")).collect();
    assert_eq!(got, [(7016, 515), (7287, 299), (0, 0), (0, 0), (0, 0)]);
}

// the same with `package.loaded` emptied: 5.3 on open every library again
#[test]
#[cfg(target_pointer_width = "64")]
fn reopening_unloaded_libraries_counts_what_puc_counts() {
    let setup = "for k in pairs(package.loaded) do package.loaded[k] = nil end";
    let got: Vec<_> = ALL.iter().map(|&v| reopen_counts(v, setup)).collect();
    let want = [
        (7016, 515),
        (7287, 299),
        (7928, 363),
        (6812, 899),
        (5344, 579),
    ];
    assert_eq!(got, want);
}

#[test]
fn globals_point_back_at_the_libraries() {
    let setup = "R = package.loaded.string; _G.string = nil; G = _G; _G = {}";
    let check = "return _G == G, string == R";
    let want = [
        "true false",
        "true false",
        "true true",
        "true true",
        "true true",
    ];
    for (v, w) in ALL.into_iter().zip(want) {
        assert_eq!(reopened(v, setup, check), w, "{v:?}");
    }
}

/// `gc-trace`: the accounting trace of a second `open_all_libs` between
/// the marks `reopen` and `/reopen`, for comparing with PUC's
/// from an instrumented host: `LUNA_REOPEN=5.x R38_TRACE=<file>`,
/// `LUNA_REOPEN_SETUP=<chunk>` run before, and with `LUNA_REOPEN_COLLECT`
/// a full collection after, between the marks `collect` and `/collect`
/// (`LUNA_REOPEN_SKIP`: no reopening between the marks).
#[test]
#[ignore]
fn reopen_trace() {
    let Ok(d) = std::env::var("LUNA_REOPEN") else {
        return;
    };
    let v = ALL[["5.1", "5.2", "5.3", "5.4", "5.5"]
        .iter()
        .position(|&x| x == d)
        .expect("dialect")];
    let mut vm = lua_c_opened(v);
    // as the host does it: the collector stopped, the setup chunk run as
    // `luaL_dostring` runs it
    vm.host_gc(0, [0; 3]).expect("stop");
    let setup = std::env::var("LUNA_REOPEN_SETUP").unwrap_or_default();
    let f = vm.load(setup.as_bytes(), setup.as_bytes()).expect("setup");
    vm.call_value(Value::Closure(f), &[]).expect("setup");
    vm.trace_mark("reopen");
    if std::env::var_os("LUNA_REOPEN_SKIP").is_none_or(|v| v.is_empty()) {
        vm.open_all_libs().expect("reopen");
    }
    vm.trace_mark("/reopen");
    if std::env::var_os("LUNA_REOPEN_COLLECT").is_some_and(|v| !v.is_empty()) {
        vm.trace_mark("collect");
        vm.host_gc(2, [0; 3]).expect("collect");
        vm.trace_mark("/collect");
    }
}

#[test]
fn libraries_reopened_after_package_loaded_emptied_are_the_globals() {
    let setup = "OLD = {s = string, m = math}; for k in pairs(package.loaded) do package.loaded[k] = nil end";
    let check = r#"return string == package.loaded.string, math == package.loaded.math,
        string ~= OLD.s, math ~= OLD.m, getmetatable("").__index == string"#;
    let want = [
        "true true false false true",
        "true true true true true",
        "true true true true true",
        "true true true true true",
        "true true true true true",
    ];
    for (v, w) in ALL.into_iter().zip(want) {
        assert_eq!(reopened(v, setup, check), w, "{v:?}");
    }
}
