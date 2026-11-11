use luna_core::jit::aot_meta::AotRole;
use luna_core::version::LuaVersion;

use super::super::compile_to_dump;
use super::{select_installable, warmup_and_capture};

// a loop entered with an integer accumulator, then with a float: the
// warmup records the float trace as an entry side trace of the first
const CLIFF: &[u8] = b"local function f(x, n, ...) local s = x + select('#', ...)\n\
    for i = 1, n do s = s + i end return s end\n\
    print(f(0, 3000), f(0.5, 30000))\n";

// a root, then its entry side trace, paired with the trace each compiled
// into and not with the root by their shared head; no side trace
#[test]
fn the_warmup_gives_a_root_then_its_entry_side_trace() {
    for version in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let td = tempfile::tempdir().expect("tempdir");
        let src = td.path().join("cliff.lua");
        std::fs::write(&src, CLIFF).expect("write source");
        let dump = compile_to_dump(&src, version).expect("compile source");
        let (_vm, captured) = warmup_and_capture(&dump, version).expect("warmup");
        let installable = select_installable(captured, false);
        let roles: Vec<AotRole> = installable.iter().map(|t| t.5).collect();
        assert_eq!(roles, [AotRole::Root, AotRole::EntrySide], "{version:?}");
        let (root, es) = (&installable[0], &installable[1]);
        assert!(
            root.4
                .entry_side_traces()
                .iter()
                .any(|e| luna_core::jit::send_compat::TArc::ptr_eq(e, &es.4))
        );
        assert!(
            es.4.is_entry_side() && !root.4.is_entry_side(),
            "{version:?}"
        );
        assert_ne!(root.4.entry_tags(), es.4.entry_tags(), "{version:?}");
        assert!(
            installable.iter().all(|t| t.3.side_trace_parent.is_none()),
            "{version:?}"
        );
    }
}
