//! Scaffold smoke test — invokes [`luna_aot::embed::embed_bytecode`]
//! programmatically with a tiny Lua source, asserts the produced
//! native binary exists and is non-empty.
//!
//! This test exercises the **end-to-end scaffold path** for v1.3:
//! parse → compile → dump → object-write → `cc` link → native binary
//! on disk. It does **not** execute the produced binary (the
//! scaffold's C entry only prints the embedded section length to
//! stderr; running it isn't a correctness signal for this session).
//! The follow-up session that wires the real Rust runtime stub adds a
//! second test asserting that the executed binary's stdout matches
//! the equivalent `luna foo.lua` run.

use std::fs;

use luna_aot::embed::embed_bytecode;
use luna_core::version::LuaVersion;

#[test]
fn embed_bytecode_produces_native_binary() {
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("hello.lua");
    fs::write(&src_path, b"return 1 + 2\n").expect("write source");

    let out_path = td.path().join("hello");
    let result = embed_bytecode(&src_path, &out_path, None, LuaVersion::Lua55);

    match result {
        Ok(()) => {}
        Err(e) => {
            // `cc` may not be present in stripped-down CI environments;
            // surface the error context so a missing toolchain doesn't
            // look like a luna bug.
            panic!("embed_bytecode failed: {e}");
        }
    }

    let meta = fs::metadata(&out_path).expect("output binary exists");
    assert!(meta.is_file(), "output is a regular file");
    assert!(meta.len() > 0, "output is non-empty");

    // Also assert the intermediate bytecode object survived (audit's
    // `--keep-obj` default for the scaffold so users can inspect):
    let stem = out_path.file_name().unwrap().to_str().unwrap();
    let bytecode_obj = td.path().join(format!("{stem}.luna_bytecode.o"));
    let obj_meta = fs::metadata(&bytecode_obj).expect("bytecode .o exists");
    assert!(obj_meta.len() > 0, "bytecode .o is non-empty");
}
