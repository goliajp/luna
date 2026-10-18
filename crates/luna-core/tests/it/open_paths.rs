//! A library opened alone (`Vm::open_math`, …) runs the same part of the
//! opening script `open_all_libs` runs, so its tables come out the same:
//! the same keys, which `next` gives in the same order. Compared as sets
//! only: the globals, which the sandboxed `open_base` fills without
//! `dofile` and `loadfile` (luna's `open_os_io` adds them last), and the
//! registry with the tables shared by every library (`_LOADED`), which
//! fill in the order the libraries are opened; among them the table of C
//! libraries, which 5.3 keys by an address, so where it lands in the
//! registry differs from run to run.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("lib_pairs_order/pairsorder.lua");

fn listing(vm: &mut Vm) -> Vec<(String, Vec<String>)> {
    let text = match vm.eval(SCRIPT).expect("listing").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("a listing, got {other:?}"),
    };
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        match line.strip_prefix("T ") {
            Some(name) => out.push((name.to_string(), Vec::new())),
            None => out
                .last_mut()
                .expect("a table first")
                .1
                .push(line.to_string()),
        }
    }
    out
}

/// Every library of `v` opened one at a time, in `luaL_openlibs`'s order.
fn one_by_one(v: LuaVersion) -> Vm {
    let mut vm = Vm::new_minimal_with_hash_seed(v, 1);
    vm.open_base().expect("base");
    vm.open_package().expect("package");
    vm.open_coroutine().expect("coroutine");
    vm.open_table().expect("table");
    vm.open_os_io().expect("io, os");
    vm.open_string().expect("string");
    if matches!(v, LuaVersion::Lua52 | LuaVersion::Lua53) {
        vm.open_bit32().expect("bit32");
    }
    vm.open_math().expect("math");
    vm.open_utf8().expect("utf8");
    vm.open_debug().expect("debug");
    vm
}

#[test]
fn libraries_opened_alone_have_the_tables_all_at_once_has() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut all = Vm::new_minimal_with_hash_seed(v, 1);
        all.open_all_libs().expect("libraries");
        let (a, b) = (listing(&mut all), listing(&mut one_by_one(v)));
        let names = |l: &[(String, Vec<String>)]| l.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&a), names(&b), "{v:?}: the tables");
        for ((name, x), (_, y)) in a.iter().zip(&b) {
            let shared = ["_G", "registry", "registry._LOADED", "package.loaded"];
            if shared.contains(&name.as_str()) {
                let set = |k: &[String]| {
                    let mut k = k.to_vec();
                    k.sort();
                    k
                };
                assert_eq!(set(x), set(y), "{v:?}: the keys of {name}");
            } else {
                assert_eq!(x, y, "{v:?}: {name}");
            }
        }
    }
}
