//! `Vm::take_error_traceback`: the traceback of an error that reaches the
//! host, in PUC's text. Each program under `tests/error_traceback/` runs in
//! every dialect, loaded as `@<name>.lua`; its `<name>.txt` holds, per
//! dialect, what a C host gets from PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.9 /
//! 5.5.1 when it loads the file the same way and calls it with `lua_pcall`
//! under a message handler returning `luaL_traceback(L, L, NULL, 1)` (5.1:
//! `debug.traceback("", 2)` without its leading newline). 5.2 names a C
//! function by walking the global table, in an order that changes from run
//! to run (`'_G.error'` or `'error'`); the files keep the short form, which
//! luna prints.

use std::path::PathBuf;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: [(&str, LuaVersion); 5] = [
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/error_traceback")
}

/// The expected traceback of `<name>.txt` for `dialect`.
fn expected(name: &str, dialect: &str) -> String {
    let path = fixture_dir().join(format!("{name}.txt"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}: {path:?}"));
    let head = format!("== {dialect}\n");
    let start = text.find(&head).expect("dialect section") + head.len();
    let end = text[start..]
        .find("\n== ")
        .map_or(text.len(), |i| start + i);
    text[start..end].trim_end_matches('\n').to_string()
}

fn programs() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixture_dir())
        .expect("fixture dir")
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".lua").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Run `<name>.lua` the way an embedder does and take the traceback.
fn snapshot(vm: &mut Vm, name: &str) -> String {
    let src = std::fs::read(fixture_dir().join(format!("{name}.lua"))).expect("read");
    let main = vm
        .load(&src, format!("@{name}.lua").as_bytes())
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    match vm.call_value(Value::Closure(main), &[]) {
        Ok(_) => panic!("{name}: ran without an error"),
        Err(_) => vm.take_error_traceback().expect("traceback taken"),
    }
}

#[test]
fn every_program_matches_puc() {
    let mut failures = Vec::new();
    for name in programs() {
        for (dialect, v) in DIALECTS {
            let got = snapshot(&mut Vm::new(v), &name);
            let want = expected(&name, dialect);
            if got != want {
                failures.push(format!(
                    "{name} {dialect}\n--- puc\n{want}\n--- luna\n{got}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// kevy's case: `error` called by `string.gsub`. Both C functions are
/// levels of their own, innermost first, before the Lua function that
/// called `gsub`.
#[test]
fn library_functions_are_c_levels() {
    let mut vm = Vm::new(LuaVersion::Lua51);
    assert_eq!(
        snapshot(&mut vm, "gsub_error2"),
        "stack traceback:\n\t[C]: ?\n\t[C]: in function 'gsub'\n\t\
         gsub_error2.lua:1: in function 'f'\n\tgsub_error2.lua:2: in main chunk"
    );
    let mut vm = Vm::new(LuaVersion::Lua54);
    assert_eq!(
        snapshot(&mut vm, "gsub_error2"),
        "stack traceback:\n\t[C]: in function 'error'\n\t[C]: in function 'string.gsub'\n\t\
         gsub_error2.lua:1: in local 'f'\n\tgsub_error2.lua:2: in main chunk"
    );
}

/// A native the host calls directly raises with no Lua function on the
/// stack; PUC's handler still sees it as level 1.
#[test]
fn a_native_called_by_the_host() {
    for (dialect, v) in DIALECTS {
        let mut vm = Vm::new(v);
        let error = vm.globals().get(Value::Str(vm.heap.intern(b"error")));
        let msg = Value::Str(vm.heap.intern(b"boom"));
        vm.call_value(error, &[msg]).unwrap_err();
        let want = if v == LuaVersion::Lua51 {
            "stack traceback:\n\t[C]: ?"
        } else {
            "stack traceback:\n\t[C]: in function 'error'"
        };
        assert_eq!(
            vm.take_error_traceback().as_deref(),
            Some(want),
            "{dialect}"
        );
    }
}

/// The traceback is taken once per error and cleared by the next call; an
/// error a Lua `pcall` catches leaves none.
#[test]
fn taken_once_and_not_for_caught_errors() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.eval("error('x')").unwrap_err();
    assert!(vm.take_error_traceback().is_some());
    assert!(vm.take_error_traceback().is_none());
    vm.eval("error('x')").unwrap_err();
    vm.eval("return 1").unwrap();
    assert!(vm.take_error_traceback().is_none());
    vm.eval("pcall(error, 'x')").unwrap();
    assert!(vm.take_error_traceback().is_none());
}
