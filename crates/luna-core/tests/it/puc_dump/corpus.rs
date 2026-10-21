//! The fixture corpus the dump tests run, one test per dialect and shard so
//! a slow build (`gc-stress` collects at every checkpoint) runs them in
//! parallel.

use std::cell::Cell;
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

/// Fixtures a collection at every checkpoint makes too slow for the merge
/// check (5.2 checks at every call, and these fill the stack): each runs
/// as its own test, collecting at every `n`th checkpoint, `n` the least
/// that keeps it within ten minutes on one of firefly's big cores (see
/// [`slow_fixture`] for full strength). (test, dialect, fixture, n)
const SLOW: &[(&str, &str, &str, u64)] = &[
    (
        "dump_round_trips_through_luna",
        "5.2",
        "1047_msgh_at_full_stack.lua",
        1,
    ),
    (
        "dump_round_trips_through_luna",
        "5.2",
        "870_c_stack_overflow_paths.lua",
        1,
    ),
];

thread_local! {
    /// The interval and offset the running fixture's Vms collect at.
    static INTERVAL: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

/// Set up `vm` as the running fixture wants it collected.
pub(super) fn stressed(vm: &mut luna_core::vm::Vm) {
    #[cfg(feature = "gc-stress")]
    if let Some((every, offset)) = INTERVAL.get() {
        vm.gc_stress_interval(every, offset);
    }
    let _ = vm;
}

fn is_slow(name: &str, dialect: &str, f: &Path) -> bool {
    SLOW.iter()
        .any(|&(t, d, file, _)| t == name && d == dialect && f.ends_with(file))
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

/// Run `check` on shard `shard` of dialect `DIALECTS[dialect]`'s fixtures,
/// the slow ones left to their own tests.
pub(super) fn each_fixture(name: &str, dialect: usize, shard: usize, check: Check) {
    let d = DIALECTS[dialect].0;
    let all: Vec<PathBuf> = fixtures(d)
        .into_iter()
        .filter(|f| !is_slow(name, d, f))
        .enumerate()
        .filter(|(i, _)| i % SHARDS == shard)
        .map(|(_, f)| f)
        .collect();
    assert!(!all.is_empty(), "[{name}] {d} shard {shard}: no fixtures");
    run(name, dialect, &format!("{d}/{shard}"), &all, check);
}

/// Run `check` on the slow fixture `file` of `DIALECTS[dialect]` at the
/// interval [`SLOW`] gives it, from an offset a seed picks: the seed is
/// printed, and `LUNA_GC_STRESS_SEED` runs it again. With
/// `LUNA_GC_STRESS_EVERY=k` and `LUNA_GC_STRESS_OFFSET=i` it collects at
/// the checkpoints `i`, `i + k`, …: the offsets `0..k` together collect at
/// every one once (`k` 1 is full strength).
pub(super) fn slow_fixture(name: &str, dialect: usize, file: &str, check: Check) {
    let d = DIALECTS[dialect].0;
    let &(_, _, _, every) = SLOW
        .iter()
        .find(|&&(t, sd, f, _)| t == name && sd == d && f == file)
        .expect("a fixture SLOW lists");
    let num = |var: &str| {
        std::env::var(var)
            .ok()
            .map(|n| n.parse::<u64>().expect("a number"))
    };
    let every = num("LUNA_GC_STRESS_EVERY").unwrap_or(every);
    let offset = num("LUNA_GC_STRESS_OFFSET").unwrap_or_else(|| {
        let seed = num("LUNA_GC_STRESS_SEED").unwrap_or_else(|| {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
            now.expect("a clock after 1970").as_nanos() as u64
        });
        eprintln!("[{name}] {d}/{file}: LUNA_GC_STRESS_SEED={seed}");
        seed % every
    });
    if cfg!(feature = "gc-stress") {
        eprintln!("[{name}] {d}/{file}: every {every}th checkpoint from the {offset}th");
        INTERVAL.set(Some((every, offset)));
    }
    let f = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/diff_puc")
        .join(d)
        .join(file);
    run(name, dialect, &format!("{d}/{file}"), &[f], check);
    INTERVAL.set(None);
}

/// Run `check` on `all`; fail on any miss. Under `gc-stress` each
/// fixture's time is printed as it finishes.
fn run(name: &str, dialect: usize, label: &str, all: &[PathBuf], check: Check) {
    let (d, version, lua_key, luac_key) = DIALECTS[dialect];
    let (Ok(lua), luac) = (std::env::var(lua_key), std::env::var(luac_key)) else {
        assert!(!require_all(), "[{name}] {lua_key} must be set");
        eprintln!("[{name}] {d}: SKIPPED — {lua_key} not set");
        return;
    };
    let luac = luac.unwrap_or_default();
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
                "[{name}] {label} {}/{} {:.1}s {:.1}s {}",
                i + 1,
                all.len(),
                t.elapsed().as_secs_f64(),
                start.elapsed().as_secs_f64(),
                f.display()
            );
        }
    }
    eprintln!(
        "[{name}] {label}: passed {}/{}",
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
/// [`DIALECTS`]) and each shard, running `$check`; `$name::slow::$sv` one
/// for each slow fixture `$sf` of dialect `$sd`.
macro_rules! corpus_tests {
    ($name:ident, $check:path, $($v:ident = $d:literal),+
     $(; slow $($sv:ident = $sd:literal $sf:literal),+)?) => {
        mod $name {
            $(
                mod slow {
                    use super::super::corpus::slow_fixture;
                    const NAME: &str = stringify!($name);
                    $(#[test] fn $sv() { slow_fixture(NAME, $sd, $sf, $check) })+
                }
            )?
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
