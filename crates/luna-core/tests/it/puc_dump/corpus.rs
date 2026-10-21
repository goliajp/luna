//! The fixture corpus the dump tests run, one test per dialect and shard so
//! a slow build (`gc-stress` collects at every checkpoint) runs them in
//! parallel.

use std::path::{Path, PathBuf};
use std::time::Instant;

use luna_core::version::LuaVersion;

pub(super) const DIALECTS: &[(&str, LuaVersion, &str, &str)] = &[
    ("5.1", LuaVersion::Lua51, "PUC_LUA_51", "PUC_LUAC_51"),
    ("5.2", LuaVersion::Lua52, "PUC_LUA_52", "PUC_LUAC_52"),
    ("5.3", LuaVersion::Lua53, "PUC_LUA_53", "PUC_LUAC_53"),
    ("5.4", LuaVersion::Lua54, "PUC_LUA_54", "PUC_LUAC_54"),
    ("5.5", LuaVersion::Lua55, "PUC_LUA_55", "PUC_LUAC_55"),
];

/// Each dialect's fixtures are split this many ways, the `i`th fixture
/// going to shard `i % SHARDS`.
pub(super) const SHARDS: usize = 8;

/// One fixture's check: its path, the dialect, PUC's `lua` and `luac`
/// (empty when not set) and the source.
pub(super) type Check = fn(&Path, LuaVersion, &str, &str, &str) -> Result<(), String>;

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
    // a count fixture reads `collectgarbage("count")`, and some fixtures
    // where the automatic steps run, both of which loading a dump instead
    // of the text changes
    out.retain(|p| {
        p.extension().is_some_and(|x| x == "lua")
            && !p.to_string_lossy().contains("_count")
            && !super::super::diff_puc::bytecode::follows_automatic_steps(p)
    });
    out.sort();
    out
}

/// Run `check` on shard `shard` of dialect `DIALECTS[dialect]`'s fixtures;
/// fail on any miss. Under `gc-stress` each fixture's time is printed as it
/// finishes.
pub(super) fn each_fixture(name: &str, dialect: usize, shard: usize, check: Check) {
    let (d, version, lua_key, luac_key) = DIALECTS[dialect];
    let (Ok(lua), luac) = (std::env::var(lua_key), std::env::var(luac_key)) else {
        assert!(!require_all(), "[{name}] {lua_key} must be set");
        eprintln!("[{name}] {d}: SKIPPED — {lua_key} not set");
        return;
    };
    let luac = luac.unwrap_or_default();
    let all: Vec<PathBuf> = fixtures(d)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % SHARDS == shard)
        .map(|(_, f)| f)
        .collect();
    assert!(!all.is_empty(), "[{name}] {d} shard {shard}: no fixtures");
    let mut failed = Vec::new();
    let start = Instant::now();
    for (i, f) in all.iter().enumerate() {
        let t = Instant::now();
        let source = std::fs::read_to_string(f).expect("read fixture");
        if let Err(e) = check(f, version, &lua, &luac, &source) {
            failed.push(format!("{}: {e}", f.display()));
        }
        if cfg!(feature = "gc-stress") {
            eprintln!(
                "[{name}] {d}/{shard} {}/{} {:.1}s {:.1}s {}",
                i + 1,
                all.len(),
                t.elapsed().as_secs_f64(),
                start.elapsed().as_secs_f64(),
                f.display()
            );
        }
    }
    eprintln!(
        "[{name}] {d} shard {shard}: passed {}/{}",
        all.len() - failed.len(),
        all.len()
    );
    assert!(
        failed.is_empty(),
        "[{name}] {} fixtures fail:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

/// `$name::$v::s<k>`: one test for each listed dialect (an index into
/// [`DIALECTS`]) and each shard, running `$check`.
macro_rules! corpus_tests {
    ($name:ident, $check:path, $($v:ident = $d:literal),+) => {
        mod $name {
            $(
                mod $v {
                    use super::super::corpus::each_fixture;
                    const NAME: &str = stringify!($name);
                    #[test] fn s0() { each_fixture(NAME, $d, 0, $check) }
                    #[test] fn s1() { each_fixture(NAME, $d, 1, $check) }
                    #[test] fn s2() { each_fixture(NAME, $d, 2, $check) }
                    #[test] fn s3() { each_fixture(NAME, $d, 3, $check) }
                    #[test] fn s4() { each_fixture(NAME, $d, 4, $check) }
                    #[test] fn s5() { each_fixture(NAME, $d, 5, $check) }
                    #[test] fn s6() { each_fixture(NAME, $d, 6, $check) }
                    #[test] fn s7() { each_fixture(NAME, $d, 7, $check) }
                }
            )+
        }
    };
}
pub(super) use corpus_tests;
