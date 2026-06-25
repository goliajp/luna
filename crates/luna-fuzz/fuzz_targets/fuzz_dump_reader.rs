//! v2.0 Track CV fuzz harness — bytecode dump reader (luna + PUC 5.1-5.5).
//!
//! Feeds arbitrary bytes into `luna_core::vm::dump::undump`. The reader
//! is the largest unsafe-deserialisation surface in luna-core (5 PUC
//! dialect parsers + luna's own format). Any panic / out-of-bounds /
//! infinite-loop is a real bug.
//!
//! Run:
//!     cargo +nightly fuzz run fuzz_dump_reader

#![no_main]

use libfuzzer_sys::fuzz_target;
use luna_core::runtime::heap::Heap;
use luna_core::version::LuaVersion;
use luna_core::vm::dump;

const VERSIONS: &[LuaVersion] = &[
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fuzz_target!(|data: &[u8]| {
    if data.len() > 256 * 1024 {
        return;
    }
    // allow_puc=true exercises the full router (luna's own format
    // detected by version byte, PUC fallback otherwise). Each iteration
    // gets a fresh Heap so cross-call state pollution can't mask bugs.
    for &ver in VERSIONS {
        let mut heap = Heap::new();
        let _ = dump::undump(data, &mut heap, ver, true);
    }
});
