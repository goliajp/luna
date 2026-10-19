//! The corpus the trace tiers are compared on: the differential fixtures,
//! run under the interpreter and under traces, their output captured.

use std::path::{Path, PathBuf};

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

pub(super) const DIALECTS: &[(&str, LuaVersion)] = &[
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

pub(super) fn fixtures(dialect: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../luna-core/tests/diff_puc")
        .join(dialect);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    out.sort();
    out
}

/// `source`'s output with `print` / `io.write` captured, or its error;
/// table and function addresses masked.
pub(super) fn run(vm: &mut Vm, source: &[u8]) -> Result<String, String> {
    vm.eval(
        r#"
_G.__out = {}
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out[#_G.__out + 1] = table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    for i = 1, select('#', ...) do _G.__out[#_G.__out + 1] = tostring(select(i, ...)) end
end
"#,
    )
    .expect("capture preamble");
    let r = match vm.load(source, b"=t") {
        Err(e) => Err(String::from_utf8_lossy(&e.msg).into_owned()),
        Ok(f) => match vm.call_value(Value::Closure(f), &[]) {
            Err(e) => Err(vm.error_display(&e)),
            Ok(_) => match vm
                .eval("return table.concat(_G.__out)")
                .expect("buffer")
                .first()
            {
                Some(Value::Str(s)) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
                other => panic!("expected the capture buffer, got {other:?}"),
            },
        },
    };
    r.map(|s| mask(&s)).map_err(|s| mask(&s))
}

/// Addresses (`table: 0x…`, on Windows 16 upper-case hex digits) differ
/// between two Vms.
pub(super) fn mask(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let hex_run = |from: usize| {
            b[from..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count()
        };
        let n = if b[i..].starts_with(b"0x") {
            2 + hex_run(i + 2)
        } else {
            0
        };
        let windows = (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b[i..]
                .iter()
                .take_while(|c| {
                    c.is_ascii_digit() || c.is_ascii_uppercase() && c.is_ascii_hexdigit()
                })
                .count()
                == 16
            && b.get(i + 16).is_none_or(|c| !c.is_ascii_alphanumeric());
        if n > 2 || windows {
            out.push_str("ADDR");
            i += if windows { 16 } else { n };
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// A Vm with the JIT and the libraries, its strings hashed with `seed`:
/// where a collection falls depends on how keys collide (5.2 and 5.3 sweep
/// strings by bucket), so the runs compared share one.
pub(super) fn seeded(version: LuaVersion, seed: u32) -> Vm {
    let mut vm = Vm::new_minimal_with_hash_seed(version, seed);
    luna_jit::install_default_jit(&mut vm);
    vm.open_all_libs().expect("the libraries open");
    vm
}

pub(super) fn interp(version: LuaVersion) -> Vm {
    interp_seeded(version, 1)
}

pub(super) fn interp_seeded(version: LuaVersion, seed: u32) -> Vm {
    let mut vm = seeded(version, seed);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

/// A trace-JIT Vm recording loops after `hot` back edges, in `tier`;
/// `tier_up_at` for [`TraceTier::Auto`].
pub(super) fn tiered(version: LuaVersion, hot: u32, tier: TraceTier, tier_up_at: u32) -> Vm {
    tiered_seeded(version, 1, hot, tier, tier_up_at)
}

pub(super) fn tiered_seeded(
    version: LuaVersion,
    seed: u32,
    hot: u32,
    tier: TraceTier,
    tier_up_at: u32,
) -> Vm {
    let mut vm = seeded(version, seed);
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = hot;
    vm.jit.call_hot_threshold = hot;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

/// What a corpus run did: trace entries, traces moved to Cranelift.
#[derive(Default)]
pub(super) struct Seen {
    pub(super) dispatched: u64,
    pub(super) tiered_up: u64,
}

pub(super) fn corpus(tier: TraceTier, tier_up_at: u32) -> Seen {
    corpus_seeded(tier, tier_up_at, |_| 1)
}

/// [`corpus`] with the strings of dialect number `i`'s runs hashed with
/// `seed(i)`.
pub(super) fn corpus_seeded(tier: TraceTier, tier_up_at: u32, seed: impl Fn(usize) -> u32) -> Seen {
    let mut failed = Vec::new();
    let mut seen = Seen::default();
    for (i, &(dialect, version)) in DIALECTS.iter().enumerate() {
        let seed = seed(i);
        for f in &fixtures(dialect) {
            let source = std::fs::read(f).expect("read fixture");
            let want = run(&mut interp_seeded(version, seed), &source);
            for hot in [1, 3] {
                let mut vm = tiered_seeded(version, seed, hot, tier, tier_up_at);
                let got = run(&mut vm, &source);
                seen.dispatched += vm.trace_dispatched_count();
                seen.tiered_up += vm.trace_tiered_up_count();
                if got != want {
                    failed.push(format!(
                        "{} (hot {hot}):\n  interpreter: {want:?}\n  traces:      {got:?}",
                        f.display()
                    ));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(seen.dispatched > 0, "no trace ran, so nothing was compared");
    seen
}
