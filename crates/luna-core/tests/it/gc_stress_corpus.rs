//! The diff_puc corpus with a full collection at every point PUC checks
//! for one (`gc-stress`), on memory that is never reused and is filled
//! when freed: an object the runtime holds only in a Rust local across
//! such a point, and frees there, shows up as a freed block written, as
//! the fill in the output, or as a crash. Each fixture's output on such
//! a Vm must be free of the fill when its output on an ordinary Vm is.
//!
//! `LUNA_STRESS_CORPUS=<dialect>[:<name part>]` runs one dialect, or the
//! fixtures of it whose names hold the part.

use std::path::PathBuf;

use luna_core::version::LuaVersion;

use super::refusing_alloc::run;

/// Fixtures too slow for a collection at every checkpoint in one job:
/// the stress-slow workflow runs them in shards.
const SLOW: &[(&str, &str)] = &[
    ("5.2", "870_c_stack_overflow_paths"),
    ("5.2", "1047_msgh_at_full_stack"),
    ("5.2", "999_handler_after_varinfo_52"),
];

const DIALECTS: &[(&str, LuaVersion)] = &[
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

/// The fixture `src` run with its output captured: what it printed, then
/// how it ended.
fn wrapped(src: &str) -> Option<String> {
    let close = "]=======]";
    if src.contains(close) {
        return None;
    }
    Some(format!(
        "local out = {{}}
         local function put(...)
           for i = 1, select('#', ...) do out[#out + 1] = tostring((select(i, ...))) end
         end
         print = function(...) put(...) out[#out + 1] = '\\n' end
         io.write = function(...) put(...) end
         local f, e = (loadstring or load)([=======[{src}{close}, '=fixture')
         local ok
         if f then ok, e = pcall(f) end
         return table.concat(out) .. '|' .. tostring(e)"
    ))
}

/// `source`'s result on an ordinary Vm, read as [`run`] reads it.
fn plain(v: LuaVersion, source: &str) -> String {
    let mut vm = luna_core::vm::Vm::new(v);
    match vm.eval(source).ok().and_then(|r| r.first().copied()) {
        Some(luna_core::runtime::Value::Str(s)) => {
            String::from_utf8_lossy(s.as_bytes()).into_owned()
        }
        _ => String::new(),
    }
}

fn fixtures(dialect: &str, part: &str) -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/diff_puc")
        .join(dialect);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the fixtures")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .filter(|p| p.to_string_lossy().contains(part))
        .collect();
    out.sort();
    out
}

#[test]
#[ignore = "slow: the gc-stress-corpus workflow runs it, a dialect a job"]
fn the_corpus_reads_and_writes_nothing_freed() {
    let only = std::env::var("LUNA_STRESS_CORPUS").unwrap_or_default();
    let (want_d, part) = only.split_once(':').unwrap_or((&only, ""));
    let mut bad = Vec::new();
    let mut ran = 0;
    for &(d, v) in DIALECTS {
        if !want_d.is_empty() && want_d != d {
            continue;
        }
        for f in fixtures(d, part) {
            if SLOW
                .iter()
                .any(|&(sd, n)| sd == d && f.to_string_lossy().contains(n))
            {
                continue;
            }
            let src = std::fs::read_to_string(&f).expect("a fixture");
            let Some(source) = wrapped(&src) else {
                continue;
            };
            let t = std::time::Instant::now();
            let got = run(v, 1, usize::MAX, &source);
            eprintln!("{d} {} {:.1}s", f.display(), t.elapsed().as_secs_f64());
            ran += 1;
            // bytes no UTF-8 reads (the fill among them), as many as the
            // fixture prints on an ordinary Vm
            let odd = |s: &str| s.matches('\u{FFFD}').count();
            if got.starts_with("a freed block written") || odd(&got) > odd(&plain(v, &source)) {
                let head: String = got.chars().take(200).collect();
                bad.push(format!("{d} {}: {head}", f.display()));
            }
        }
    }
    assert!(ran > 0, "no fixture ran");
    assert!(
        bad.is_empty(),
        "{} of {ran}:\n{}",
        bad.len(),
        bad.join("\n")
    );
}
