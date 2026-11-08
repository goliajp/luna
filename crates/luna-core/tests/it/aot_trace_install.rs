//! `__private::install_aot_trace` + `Vm::collect_proto_hashes` smoke tests.
//!
//! These cover the install API surface only — the deploy-side
//! resolver that *calls* this API lives in `luna-runtime-helpers` and
//! is exercised by `crates/luna-aot/tests/it/aot_trace_fires.rs`.

use luna_core::__private::{
    CompiledTrace, CompiledTraceParts, PassCount, ProtoTraceInternals, TracePlans,
};
use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::jit::send_compat::{TArc, TCellU32};
use luna_core::jit::trace::{ExitTag, ExitTagTable};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// A no-op trace entry. Never invoked by these tests — they only
/// inspect dispatch installation, not dispatch firing.
///
/// SAFETY: callers don't run it. If a future test does, the body must
/// read its `reg_state` only within bounds the caller passes.
unsafe extern "C" fn dummy_entry(_reg_state: *mut i64) -> i64 {
    0
}

fn make_dummy_trace(head_pc: u32, max_stack: u32) -> CompiledTrace {
    install(dummy_parts(head_pc, max_stack))
}

fn dummy_parts(head_pc: u32, max_stack: u32) -> CompiledTraceParts {
    let exit_tags = vec![ExitTag::Untouched; max_stack as usize];
    let entry_tags = vec![0u8; max_stack as usize];
    CompiledTraceParts {
        head_pc,
        entry: dummy_entry,
        n_ops: 1,
        dispatchable: true,
        window_size: max_stack,
        plans: TracePlans::new(
            &entry_tags,
            &exit_tags,
            ExitTagTable::default(),
            ExitTagTable::default(),
            PassCount::Shared,
        ),
        per_exit_inline: TArc::from(Vec::new()),
        exit_hit_counts: TArc::from(vec![TCellU32::new(0u32)]),
        is_inline_abort_close: false,
        dispatch_off_reason: None,
        sinkable_sites_seen: 0,
        accum_bufferable_seen: 0,
        sunk_alloc_seen: 0,
        materialize_emit_count: 0,
        closure_seen: 0,
        inline_kinds: 0,
        inline_depth: 0,
        body_writes: Box::new([]),
        downrec_link: None,
        downrec_multi_way_count: 0,
        tier_up: None,
        side_cell_exits: std::sync::Arc::new([]),
        side_cells: None,
        link_pc: None,
        loop_tags: None,
    }
}

/// `parts` made into a trace; a debug build checks the plans are of them.
fn install(parts: CompiledTraceParts) -> CompiledTrace {
    // SAFETY: the entry has the `TraceFn` ABI and no test enters it; the one
    // test passing plans of other code runs only where the debug check stops
    // `from_parts` before a trace is made
    unsafe { CompiledTrace::from_parts(parts) }
}

#[test]
fn collect_proto_hashes_walks_tree() {
    // Compile a chunk that contains nested closures so the proto tree
    // has more than one node.
    let src = "local function f() return 1 end\n\
               local function g() return 2 end\n\
               return f, g";
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    // SAFETY: the heap outlives every handle made from it here, and its objects stay in it
    let mut heap = unsafe { luna_core::__private::new_heap() };
    let root = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");

    let vm = Vm::new(LuaVersion::Lua55);
    let hashes = vm.collect_proto_hashes(root);
    // Root + 2 nested closures = 3 entries.
    assert_eq!(hashes.len(), 3, "expected root + 2 nested protos");
    // All hashes distinct.
    let unique: std::collections::HashSet<[u8; 16]> = hashes.iter().map(|(_, h)| *h).collect();
    assert_eq!(unique.len(), 3, "proto hashes must be distinct");
}

#[test]
fn install_aot_trace_pushes_onto_proto_traces() {
    let src = "return 1";
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    // SAFETY: the heap outlives every handle made from it here, and its objects stay in it
    let mut heap = unsafe { luna_core::__private::new_heap() };
    let root = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");

    assert_eq!(root.traces().len(), 0, "fresh Proto starts with no traces");

    let mut vm = Vm::new(LuaVersion::Lua55);
    let trace = make_dummy_trace(0, root.max_stack as u32);
    // SAFETY: the trace is never entered: nothing runs `root`
    unsafe { luna_core::__private::install_aot_trace(&mut vm, root, trace) };

    let traces = root.traces();
    assert_eq!(traces.len(), 1, "install must push exactly one trace");
    assert_eq!(traces[0].head_pc(), 0);
    assert!(
        traces[0].dispatchable(),
        "dummy trace was marked dispatchable"
    );
}

#[test]
fn install_then_lookup_via_collect_hashes() {
    // End-to-end install API exercise that mirrors how the deploy
    // resolver will use it: hash the loaded proto, find it in the
    // collected (proto, hash) pairs, install a trace onto it.
    let src = "local s = 0\nfor i = 1, 5 do s = s + 1 end\nreturn s";
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    // SAFETY: the heap outlives every handle made from it here, and its objects stay in it
    let mut heap = unsafe { luna_core::__private::new_heap() };
    let root = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");
    let target_hash = root.stable_hash();

    let mut vm = Vm::new(LuaVersion::Lua55);
    let hashes = vm.collect_proto_hashes(root);
    let matched = hashes
        .iter()
        .find(|(_, h)| *h == target_hash)
        .map(|(p, _)| *p)
        .expect("root proto must be present in collected pairs");

    let trace = make_dummy_trace(3, root.max_stack as u32);
    // SAFETY: as above
    unsafe { luna_core::__private::install_aot_trace(&mut vm, matched, trace) };

    assert_eq!(
        root.traces().len(),
        1,
        "trace must end up on the matched proto"
    );
}

/// Plans made for more inline exits than the parts have are not of this
/// code: debug builds stop there.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "plans are not of its code")]
fn plans_of_other_code_are_caught_in_debug_builds() {
    let mut parts = dummy_parts(0, 2);
    // two inline exits, which the parts do not have
    let tags = vec![ExitTag::Untouched; 2];
    let inline = ExitTagTable::from_exits([(0, &tags[..]), (1, &tags[..])]);
    let plans = TracePlans::new(
        &[0, 0],
        &tags,
        ExitTagTable::default(),
        inline,
        PassCount::Shared,
    );
    parts.plans = plans;
    let _ = install(parts);
}
