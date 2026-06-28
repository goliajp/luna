//! v2.0 Track CV fuzz harness — Lua source parser.
//!
//! Feeds arbitrary bytes into `luna_core::frontend::parser::parse` for
//! every dialect we support and asserts the parser must either return
//! `Ok(Chunk)` or `Err(SyntaxError)` — **panicking is a real bug**
//! (per `code/no-blind-bugfix-pattern`, we don't try-catch around
//! the call). libfuzzer-sys traps panics natively.
//!
//! Run:
//!     cargo +nightly fuzz run fuzz_parser
//!
//! Per-track content fill: seed corpus from `tests/official/*.lua` +
//! crash inputs from any panics this harness uncovers.

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::frontend::parser;
use luna_core::version::LuaVersion;

const VERSIONS: &[LuaVersion] = &[
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::MacroLua,
    LuaVersion::Lua55,
];

fuzz_target!(|data: &[u8]| {
    // Bound: parser is supposed to handle arbitrary byte input. Cap
    // at 64 KiB so libfuzzer rounds stay productive (large inputs
    // dominate runtime without exercising new state).
    if data.len() > 64 * 1024 {
        return;
    }
    for &ver in VERSIONS {
        // Result intentionally ignored — we only care that parse does
        // not panic / abort / OOM. SyntaxError is a valid outcome.
        let _ = parser::parse(data, ver);
    }
});
