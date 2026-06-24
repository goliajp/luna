//! v1.3 Phase AOT Stage 7 sub-piece 4 — `Proto::stable_hash` tests.
//!
//! See `crates/luna-core/src/runtime/function.rs` (the
//! `Proto::stable_hash` docstring) for the algorithm + identity
//! contract this exercises.

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;

fn hash_of(src: &str) -> [u8; 16] {
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    let mut heap = Heap::new();
    let proto = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");
    proto.stable_hash()
}

#[test]
fn same_source_hashes_equal() {
    let src = "local s = 0\nfor i = 1, 100 do s = s + i end\nreturn s";
    let h1 = hash_of(src);
    let h2 = hash_of(src);
    assert_eq!(h1, h2, "two compiles of the same source must hash equal");
    // Non-zero: a buggy FNV that XOR'd into bits-out-of-range or
    // skipped the multiply would leak the offset basis verbatim.
    assert_ne!(h1, [0u8; 16]);
}

#[test]
fn different_source_hashes_differ() {
    let h1 = hash_of("return 1");
    let h2 = hash_of("return 2");
    assert_ne!(h1, h2, "distinct constants must produce distinct hashes");

    let h3 = hash_of("local x = 0\nreturn x");
    let h4 = hash_of("local y = 0\nreturn y");
    // `x` vs `y` is a local name; local names DON'T live in the
    // proto's bytecode-equivalent fields (just locvars debug records,
    // which `stable_hash` ignores). So these MUST hash equal.
    assert_eq!(
        h3, h4,
        "local variable rename (debug-only difference) must NOT change the hash"
    );
}

#[test]
fn comment_only_change_hashes_equal() {
    let h1 = hash_of("return 42");
    let h2 = hash_of("-- a comment\nreturn 42");
    assert_eq!(
        h1, h2,
        "adding a comment must not change the proto's stable hash"
    );
}

#[test]
fn nested_protos_have_distinct_hashes() {
    // Outer chunk wraps an inner closure. Both protos exist; their
    // hashes must differ (different code arrays + signatures).
    let src = "local function inner() return 7 end\nreturn inner()";
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    let mut heap = Heap::new();
    let outer = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");
    let outer_hash = outer.stable_hash();
    assert!(
        !outer.protos.is_empty(),
        "expected at least one nested proto"
    );
    let inner_hash = outer.protos[0].stable_hash();
    assert_ne!(
        outer_hash, inner_hash,
        "outer chunk vs nested proto must hash distinctly"
    );
}
