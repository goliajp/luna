//! v2.4 Phase Fuzz-A — parser fuzz target.
//!
//! Random byte input → `Lexer::strip_shebang_bom` → `parser::parse`
//! across all 5 Lua dialects. Asserts:
//! - no panic (parser must return `Err(SyntaxError)` instead)
//! - no UB / OOB read (the libFuzzer + ASAN runtime catches both)
//! - the parser terminates (libfuzzer's timeout per input catches
//!   infinite loops at the per-input level)
//!
//! Seed corpus lives at `fuzz/corpus/parse/` — populated from
//! `crates/luna-core/tests/diff_puc/*.lua` + a few hand-picked
//! minimal chunks (`return 1`, `do end`, `local x = 1`, etc.).
//!
//! Run locally:
//!     cd fuzz && cargo +nightly fuzz run parse -- -runs=10000
//!
//! Nightly CI: `.github/workflows/fuzz.yml` (Phase Fuzz-E).

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::frontend::lexer::Lexer;
use luna_core::frontend::parser;
use luna_core::version::LuaVersion;

fuzz_target!(|data: &[u8]| {
    // Run each input across every supported dialect — a parser
    // crash that only fires on 5.5 wouldn't surface if we only
    // tested 5.5 on a 5.4-shaped input. Each dialect is bounded
    // O(input.len()) so the total stays linear in input size.
    for version in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let stripped = Lexer::strip_shebang_bom(data);
        // Parser returns `Result<Chunk, SyntaxError>`. We don't
        // care which arm — only that neither panics / triggers UB.
        let _ = parser::parse(stripped, version);
    }
});
