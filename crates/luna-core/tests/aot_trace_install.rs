//! v1.3 Phase AOT Stage 7 sub-piece 4 — `Vm::install_aot_trace` +
//! `Vm::collect_proto_hashes` smoke tests.
//!
//! These cover the install API surface only — the deploy-side
//! resolver that *calls* this API lives in `luna-runtime-helpers` and
//! is exercised by `crates/luna-aot/tests/stage7_aot_trace_fires.rs`
//! (which is the end-to-end smoke + currently deferred per the
//! sub-piece-4 docstring).

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::jit::trace::{CompiledTrace, ExitTag, TagResKind, classify_exit_tags};
use luna_core::runtime::Heap;
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
    let exit_tags: std::rc::Rc<[ExitTag]> = (0..max_stack)
        .map(|_| ExitTag::Untouched)
        .collect::<Vec<_>>()
        .into();
    let global_kind: TagResKind = classify_exit_tags(&exit_tags);
    let entry_tags: std::rc::Rc<[u8]> = vec![0u8; max_stack as usize].into();
    CompiledTrace {
        head_pc,
        entry: dummy_entry,
        n_ops: 1,
        dispatchable: true,
        window_size: max_stack,
        exit_tags,
        global_tag_res_kind: global_kind,
        entry_tags,
        per_exit_tags: std::rc::Rc::from(Vec::<(u32, std::rc::Rc<[ExitTag]>)>::new()),
        per_exit_inline: std::rc::Rc::from(Vec::new()),
        exit_hit_counts: std::rc::Rc::from(vec![std::cell::Cell::new(0u32)]),
        exit_side_trace_ptrs: std::rc::Rc::from(vec![std::cell::Cell::new(std::ptr::null())]),
        tags_side_trace_ptrs: std::rc::Rc::from(Vec::new()),
        global_side_trace_ptr: Box::new(std::cell::Cell::new(std::ptr::null())),
        side_trace_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
        has_any_side_wired: std::cell::Cell::new(false),
        is_inline_abort_close: false,
        dispatch_off_reason: None,
        sinkable_sites_seen: 0,
        accum_bufferable_seen: 0,
        sunk_alloc_seen: 0,
        materialize_emit_count: 0,
        closure_seen: 0,
        body_writes: Box::new([]),
        downrec_link: None,
        downrec_multi_way_count: 0,
    }
}

#[test]
fn collect_proto_hashes_walks_tree() {
    // Compile a chunk that contains nested closures so the proto tree
    // has more than one node.
    let src = "local function f() return 1 end\n\
               local function g() return 2 end\n\
               return f, g";
    let ast = parse(src.as_bytes(), LuaVersion::Lua55).expect("parse");
    let mut heap = Heap::new();
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
    let mut heap = Heap::new();
    let root = compile_chunk(&ast, LuaVersion::Lua55, b"=test", &mut heap).expect("compile");

    assert_eq!(
        root.traces.borrow().len(),
        0,
        "fresh Proto starts with no traces"
    );

    let mut vm = Vm::new(LuaVersion::Lua55);
    let trace = make_dummy_trace(0, root.max_stack as u32);
    vm.install_aot_trace(root, trace);

    let traces = root.traces.borrow();
    assert_eq!(traces.len(), 1, "install must push exactly one trace");
    assert_eq!(traces[0].head_pc, 0);
    assert!(
        traces[0].dispatchable,
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
    let mut heap = Heap::new();
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
    vm.install_aot_trace(matched, trace);

    assert_eq!(
        root.traces.borrow().len(),
        1,
        "trace must end up on the matched proto"
    );
}
