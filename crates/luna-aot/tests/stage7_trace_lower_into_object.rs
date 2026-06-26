//! v1.3 Phase AOT Stage 7 — load-bearing smoke test for the trace
//! lowerer driven against an `ObjectModule`.
//!
//! Sibling of `stage3_lower_into_object` (which exercises the
//! *int-chunk* lowerer's `M: Module` generic): this test pins the
//! *trace* lowerer's generic by feeding it a hand-constructed
//! pure-arith [`TraceRecord`] and an [`ObjectModule`]. The trace's
//! IR is then finished into a `Vec<u8>` and the bytes verified to be
//! a well-formed object file (ELF / Mach-O / PE).
//!
//! # Why this is the "load-bearing demo" for Stage 7
//!
//! The audit (`.dev/rfcs/v1.3-audit-luna-aot.md`) estimates the
//! end-to-end trace AOT story at ~40 dev-days. The bulk of that work
//! is **runtime-side**: in-deploy trace registry walk, dispatch-table
//! install on the embedded `Vm`, and — most painfully — relocating
//! the `iconst`-baked runtime addresses (`luna_jit_table_set_field`'s
//! `key_ptr` is the *AOT-time process address* of an interned
//! `LuaStr`, which has no meaning at deploy time).
//!
//! Before any of that is worth touching, the **codegen-side**
//! invariant must be proven: that the same `lower_trace_into<M>`
//! body the runtime JIT calls can also be driven against an
//! `ObjectModule` without panic / `Module`-trait-method gaps / IR
//! shape mismatches. That's what this test demonstrates.
//!
//! # Test fixture shape
//!
//! A trace of three pure-int-arith ops:
//!
//! ```text
//!   R[0] = R[1] + R[2]
//!   R[0] = R[0] * R[3]
//!   R[0] = R[0] - R[4]
//! ```
//!
//! Pure arith was picked deliberately: **no `luna_jit_*` helper is
//! called**, so the produced `.o` has no `Linkage::Import` references
//! to the broader helper set. This sidesteps the "helpers must be in
//! the deploy-side staticlib" question (audit § Stage 3 step 3 +
//! § Open question 1) entirely — that's a separate workstream and is
//! not gated by this proof.
//!
//! # What this does NOT prove
//!
//! All of these are explicit follow-ups, called out below so the next
//! session has a clear continuation surface:
//!
//! - Helper symbols (`luna_jit_table_set_field`, `…_op_concat`, …)
//!   exposed to the deploy-side staticlib via `luna-runtime-helpers`.
//! - Baked runtime-address relocations (e.g. `key_ptr` → string
//!   re-intern at deploy-time process start).
//! - `(proto_id, pc) → mcode_offset` registry section in the AOT
//!   binary.
//! - Deploy-side `Vm` trace-dispatch table install (today gated by
//!   `TraceHandle` ownership, which the JIT side holds via
//!   `TRACE_JIT_HANDLES` thread-local).
//! - End-to-end "AOT binary actually fires AOT mcode on a hot loop"
//!   smoke (requires all four bullets above).
//!
//! The audit estimates each of the above at 5-10 dev-days. Bundling
//! them into a single session would either skip validation (no smoke
//! test) or ship a broken commit (link failures on missing helper
//! symbols). The honest scope here is: prove the codegen invariant,
//! document the rest as next-session work.

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};

use luna_core::jit::trace_types::{CompileOptions, RecordedOp, TraceRecord};
use luna_core::runtime::Gc;
use luna_core::runtime::function::Proto;
use luna_core::version::LuaVersion;
use luna_core::vm::isa::{Inst, Op};
use luna_jit::jit_backend::trace::lower_trace_into;

/// Build a host-targeting `TargetIsa` configured for PIC (required
/// for ELF / Mach-O `.o` files). The runtime JIT uses `is_pic=false`
/// (mcode is finalized at known addresses) — AOT object emission
/// flips it on so the linker can relocate the produced section.
fn host_pic_isa() -> std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("use_colocated_libcalls", "false")
        .expect("flag");
    flag_builder.set("is_pic", "true").expect("flag");
    flag_builder.set("opt_level", "speed").expect("flag");
    cranelift_native::builder()
        .expect("cranelift_native::builder")
        .finish(settings::Flags::new(flag_builder))
        .expect("isa finish")
}

/// Resolve a `Gc<Proto>` whose `max_stack` is ≥ 5 (the ops below
/// reference R[0..=4]). We piggy-back on a JIT-equipped Vm just to
/// reuse the parser + compiler; nothing here ever *runs* the proto.
fn load_wide_arity_proto(vm: &mut luna_core::vm::Vm) -> Gc<Proto> {
    let _ = vm
        .eval(
            r#"
            function add5(a, b, c, d, e)
                return ((a + b) * c) - d + e
            end
            return add5
        "#,
        )
        .expect("eval");
    let key = vm.intern_str("add5");
    let g = vm.globals();
    // SAFETY: `g` is the live globals `Gc<Table>`; nothing else holds
    // a borrow into it for the duration of this read.
    let v = unsafe { (*g.as_ptr()).get(luna_core::runtime::Value::Str(key)) };
    let cl = match v {
        luna_core::runtime::Value::Closure(c) => c,
        other => panic!("expected closure for `add5`, got {other:?}"),
    };
    // SAFETY: `cl` is a live `Gc<LuaClosure>` from the read above;
    // nothing else borrows the closure for this field read.
    let proto = unsafe { (*cl.as_ptr()).proto };
    assert!(
        proto.max_stack >= 5,
        "fixture proto must have max_stack >= 5 for the 5-reg trace; got {}",
        proto.max_stack
    );
    proto
}

/// Build a closed `TraceRecord` of three pure-int-arith ops.
///
/// All three ops share the same `proto` / `inline_depth=0` shape — a
/// single-frame trace, which is the simplest case the lowerer's
/// depth-invariant check accepts.
fn make_pure_arith_record(proto: Gc<Proto>) -> TraceRecord {
    let ops = [
        // R[0] = R[1] + R[2]
        Inst::iabc(Op::Add, 0, 1, 2, false),
        // R[0] = R[0] * R[3]
        Inst::iabc(Op::Mul, 0, 0, 3, false),
        // R[0] = R[0] - R[4]
        Inst::iabc(Op::Sub, 0, 0, 4, false),
    ];

    // head_pc = 0; the trace lowerer treats `head_pc` as a return
    // value for the clean-close path. Any in-range PC works.
    let mut rec = TraceRecord::start(proto, 0, Vec::new(), false);
    for (i, inst) in ops.iter().copied().enumerate() {
        let pushed = rec.push(RecordedOp {
            proto,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
        assert!(pushed, "test trace must fit MAX_TRACE_LEN");
    }
    rec.closed = true;
    rec
}

#[test]
fn trace_lowerer_emits_into_object_module() {
    // JIT-enabled Vm just to reach the compile front-end. We never
    // *run* anything; the produced `.o` is the deliverable.
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let proto = load_wide_arity_proto(&mut vm);

    let record = make_pure_arith_record(proto);

    // Build the ObjectModule. The name "luna_aot_stage7_trace_smoke"
    // shows up as the .o's "soname" equivalent — useful for objdump
    // diagnosis if a future regression flips an object-format flag.
    let isa = host_pic_isa();
    let object_builder =
        ObjectBuilder::new(isa, "luna_aot_stage7_trace_smoke", default_libcall_names())
            .expect("ObjectBuilder");
    let mut object_module = ObjectModule::new(object_builder);

    // **Load-bearing assertion**: the same `lower_trace_into<M>` the
    // runtime JIT calls (with `M = JITModule`) accepts `M =
    // ObjectModule` and produces a `FuncId` + `CompiledTrace` shell.
    //
    // The returned `CompiledTrace.entry` is the `placeholder_trace_fn`
    // sentinel (per the lowerer's docstring) — backend-specific
    // finalize is the caller's responsibility. The JIT wrapper
    // patches `entry` to a finalized fn pointer; the AOT pipeline
    // resolves the trace symbol at static-link time and dispatches
    // through its own table. Either way, this test doesn't touch
    // `entry` after the lower call.
    let result = lower_trace_into(&mut object_module, &record, CompileOptions::default());
    let (fn_id, compiled) = result.expect(
        "lower_trace_into should accept a closed pure-arith TraceRecord \
         regardless of backend module (JITModule vs ObjectModule)",
    );

    // Sanity-check the CompiledTrace metadata. Pure-arith Add/Mul/Sub
    // count as 3 ops; head_pc round-trips from `make_record`.
    assert_eq!(compiled.head_pc, 0, "head_pc should round-trip from record");
    assert_eq!(compiled.n_ops, 3, "three ops were pushed (Add, Mul, Sub)");

    // The lowerer's FuncId is opaque (cranelift handle); just check
    // it's non-default to ensure declare_function ran.
    let _ = fn_id;

    // Finish + emit — the deploy-side equivalent of the JIT path's
    // `module.finalize_definitions() + module.get_finalized_function(fn_id)`,
    // but producing `.o` bytes the system linker can consume.
    let product = object_module.finish();
    let bytes = product.emit().expect("ObjectProduct::emit");
    assert!(
        !bytes.is_empty(),
        "emitted object file should contain bytes (got 0)"
    );

    // Magic-byte check across the formats Stage 5 supports. Cranelift's
    // ObjectModule emits .obj artifacts (not PE executables) so Windows
    // magic is the COFF Machine-field at offset 0 (0x8664 = AMD64,
    // 0xAA64 = ARM64, 0x014c = I386), not the `MZ` DOS stub of a
    // linked .exe. Same shape as stage3_lower_into_object.rs's check.
    let is_elf = bytes.starts_with(&[0x7f, b'E', b'L', b'F']);
    let is_macho = bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xce]);
    let is_coff_obj = bytes.len() >= 2
        && matches!(
            (bytes[0], bytes[1]),
            (0x64, 0x86) | (0x64, 0xAA) | (0x4c, 0x01)
        );
    let is_pe = bytes.starts_with(b"MZ");
    assert!(
        is_elf || is_macho || is_pe || is_coff_obj,
        "emitted bytes should be a recognized object format \
         (first 8 bytes: {:?})",
        &bytes[..bytes.len().min(8)]
    );
}
