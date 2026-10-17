//! The order `next` gives over each table the libraries make is PUC's:
//! with the same string hash seed, the same keys land in the same nodes,
//! the collisions among them included. The expected orders come from PUC
//! (`expected.txt`, three seeds, five dialects), listed by the same script
//! this test runs.
//!
//! Left out: the `arg` table (what lua.c puts in it), and 5.3's registry,
//! whose C libraries' key is a light userdata hashed by its address, which
//! differs from run to run in PUC too: where it lands moves the keys it
//! collides with.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("lib_pairs_order/pairsorder.lua");
const EXPECTED: &str = include_str!("lib_pairs_order/expected.txt");

fn dialect(name: &str) -> LuaVersion {
    match name {
        "5.1" => LuaVersion::Lua51,
        "5.2" => LuaVersion::Lua52,
        "5.3" => LuaVersion::Lua53,
        "5.4" => LuaVersion::Lua54,
        _ => LuaVersion::Lua55,
    }
}

/// The tables of a listing, each with its keys.
fn tables(text: &str) -> Vec<(String, Vec<String>)> {
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

fn luna_listing(v: LuaVersion, seed: u32) -> String {
    let mut vm = Vm::new_minimal_with_hash_seed(v, seed);
    vm.open_all_libs().expect("libraries");
    // lua.c makes `arg` before it runs the script
    vm.eval("arg = {}").expect("arg");
    match vm.eval(SCRIPT).expect("listing").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("a listing, got {other:?}"),
    }
}

#[test]
fn library_tables_iterate_in_pucs_order() {
    let mut runs = 0;
    for section in EXPECTED.split("S ").filter(|s| !s.is_empty()) {
        let (head, body) = section.split_once('\n').expect("a header line");
        let (seed, d) = head.split_once(' ').expect("seed and dialect");
        let seed: u32 = seed.parse().expect("a seed");
        let v = dialect(d);
        let got = tables(&luna_listing(v, seed));
        let want = tables(body);
        let names = |l: &[(String, Vec<String>)]| l.iter().map(|t| t.0.clone()).collect::<Vec<_>>();
        assert_eq!(names(&got), names(&want), "seed {seed} {d}: the tables");
        for ((name, g), (_, w)) in got.iter().zip(&want) {
            if name == "arg" || (d == "5.3" && name == "registry") {
                continue;
            }
            assert_eq!(g, w, "seed {seed} {d}: {name}");
        }
        runs += 1;
    }
    assert_eq!(runs, 15, "three seeds, five dialects");
}
