//! What `load` holds on the stack below its reader at each call is PUC's:
//! its arguments, the piece kept in its reserved slot, and the parser's
//! objects (tables, closure, 5.1's prototypes) with their entries in
//! `next` order. The expected listings come from PUC (`expected.txt`,
//! three hash seeds, five dialects), made by the script this test runs.
//!
//! 5.5's anchor table keys objects (the main closure, each function's
//! constants table) by themselves: PUC hashes them by address, so their
//! order changes from run to run in PUC too. For 5.5 each table's entries
//! are compared as a set; the objects are still checked to be keyed by
//! themselves, and each constants table's entries in full.

use super::panic_free::panic_free;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = include_str!("load_reader_stack/reader.lua");
const EXPECTED: &str = include_str!("load_reader_stack/expected.txt");
const BINARY: &str = include_str!("load_reader_stack/binary.lua");
/// PUC's listing with the target's `size_t`: the chunk is its `string.dump`
#[cfg(target_pointer_width = "64")]
const BINARY_EXPECTED: &str = include_str!("load_reader_stack/binary_expected.txt");
#[cfg(target_pointer_width = "32")]
const BINARY_EXPECTED: &str = include_str!("load_reader_stack/binary_expected_32.txt");

fn dialect(name: &str) -> LuaVersion {
    match name {
        "5.1" => LuaVersion::Lua51,
        "5.2" => LuaVersion::Lua52,
        "5.3" => LuaVersion::Lua53,
        "5.4" => LuaVersion::Lua54,
        _ => LuaVersion::Lua55,
    }
}

fn listing(script: &str, v: LuaVersion, seed: u32) -> String {
    let mut vm = Vm::new_minimal_with_hash_seed(v, seed);
    vm.open_all_libs().expect("libraries");
    let r = panic_free("listing", || vm.eval(script));
    let out = match r.expect("listing").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("a listing, got {other:?}"),
    };
    // each table shown has PUC's array and hash part sizes, which its
    // collector's counts are made of: gc-verify builds check it
    out
}

/// The words of `s` split at spaces outside quotes and brackets.
fn words(s: &str) -> Vec<&str> {
    let (mut out, mut start, mut depth, mut quoted, mut escaped) =
        (Vec::new(), 0, 0i32, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '[' | '{' | '<' if !quoted => depth += 1,
            ']' | '}' | '>' if !quoted => depth -= 1,
            ' ' if !quoted && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// A line with each table's entries sorted.
fn unordered(line: &str) -> String {
    words(line)
        .into_iter()
        .map(
            |w| match w.strip_prefix("table{").and_then(|w| w.strip_suffix('}')) {
                Some(body) => {
                    let mut e = words(body);
                    e.sort_unstable();
                    format!("table{{{}}}", e.join(" "))
                }
                None => w.to_string(),
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

/// The sections of an expected listing (LF line ends): seed, dialect,
/// listing.
fn sections(expected: &str) -> Vec<(u32, &str, &str)> {
    let starts: Vec<usize> = expected
        .match_indices("S ")
        .filter(|&(i, _)| i == 0 || expected.as_bytes()[i - 1] == b'\n')
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::new();
    for (n, &at) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(expected.len());
        let (head, want) = expected[at + 2..end]
            .split_once('\n')
            .expect("a header line");
        let (seed, d) = head.split_once(' ').expect("seed and dialect");
        out.push((seed.parse().expect("a seed"), d, want.trim_end()));
    }
    out
}

#[test]
fn load_keeps_pucs_objects_below_its_reader() {
    let expected = EXPECTED.replace("\r\n", "\n");
    let mut runs = 0;
    for (seed, d, want) in sections(&expected) {
        let got = listing(SCRIPT, dialect(d), seed);
        if d == "5.5" {
            let lines = |s: &str| s.lines().map(unordered).collect::<Vec<_>>();
            assert_eq!(lines(&got), lines(want), "seed {seed} {d}");
        } else {
            assert_eq!(got, want, "seed {seed} {d}");
        }
        runs += 1;
    }
    assert_eq!(runs, 15, "three seeds, five dialects");
}

/// The same while `load` undumps a binary chunk handed over in pieces;
/// a reader is called no more once the chunk is read.
#[test]
fn load_keeps_pucs_objects_below_its_reader_undumping() {
    let expected = BINARY_EXPECTED.replace("\r\n", "\n");
    let mut runs = 0;
    for (seed, d, want) in sections(&expected) {
        assert_eq!(listing(BINARY, dialect(d), seed), want, "{d}");
        runs += 1;
    }
    assert_eq!(runs, 5, "five dialects");
}

/// A reader that makes objects with finalizers (a table with `__gc`, the
/// box of a `luaL_Buffer` past its room on the C stack) while `load` sets
/// what the reader makes aside: each joins the finalizer list when PUC
/// makes it, and the load and a collection go on.
#[test]
fn a_reader_makes_objects_with_finalizers() {
    const SCRIPT: &str = r#"
        local n, fin = 0, 0
        local f = load(function()
          n = n + 1
          setmetatable({}, {__gc = function() fin = fin + 1 end})
          local s = table.concat({string.rep("x", 3000), "y"})
          return ({"return ", tostring(string.len(s))})[n]
        end)
        collectgarbage()
        collectgarbage()
        return f() .. " " .. fin
    "#;
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        let r = panic_free("script", || vm.eval(SCRIPT));
        let got = match r.expect("script").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{v:?}: got {other:?}"),
        };
        // as PUC 5.2.4 to 5.5.1 give it
        assert_eq!(got, "3001 3", "{v:?}");
    }
}

/// `gc-verify` takes what a load sets aside (what its reader makes) as
/// live: with every check collecting, the reader's strings on the stack
/// are no dead values.
#[cfg(all(feature = "gc-stress", feature = "gc-verify"))]
#[test]
fn a_collection_in_a_reader_finds_what_it_made_live() {
    const SCRIPT: &str = "local n = 0
        local f = load(function()
          n = n + 1
          local s = string.rep('x', n) .. n .. 'y'
          return ({'return ', '#\"' .. s .. '\"'})[n]
        end)
        return f()";
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        let got = panic_free("script", || vm.eval(SCRIPT)).expect("script");
        let four = matches!(got.first(), Some(Value::Int(4)))
            || matches!(got.first(), Some(Value::Float(f)) if *f == 4.0);
        assert!(four, "{v:?}: {got:?}");
    }
}
