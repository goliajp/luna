//! v1.3 Phase AOT Stage 3 — smoke test for the shared int-chunk
//! lowerer. Proves that
//! [`luna_jit::jit_backend::lower_int_chunk_into`] is genuinely
//! backend-agnostic by driving it with an `ObjectModule`
//! (deploy-side `.o` emission) instead of the runtime-JIT
//! `JITModule` (live RWX mmap).
//!
//! Without this test, "generic over `M: Module`" is a claim with no
//! second consumer. With it, the AOT pipeline's contract (Stage 4+)
//! has a working compile-time witness: feed an `ObjectModule` to
//! `lower_int_chunk_into`, drive it through `finish() -> emit()`,
//! get a `Vec<u8>` containing a valid object file. The linker step
//! is out of scope for this smoke test (that's Stage 6); we assert
//! only the lowerer + object-module pipeline cleanly produces
//! bytes.
//!
//! Test fixture is the simplest possible Lua chunk that JIT-compiles
//! cleanly under the int-chunk whitelist:
//!
//! ```lua
//! local function add(a, b) return a + b end
//! ```
//!
//! Without arguments the chunk produces nothing JIT-able. With a
//! single-arg / two-arg int-arith Proto we hit the same whitelist
//! the runtime JIT uses.

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

use luna_core::version::LuaVersion;
use luna_jit::jit_backend::lower_int_chunk_into;

fn host_isa() -> std::sync::Arc<dyn cranelift_codegen::isa::TargetIsa> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("use_colocated_libcalls", "false")
        .expect("flag");
    // PIC required for ELF .o files; the JIT side sets false because
    // its mcode is finalized at known addresses.
    flag_builder.set("is_pic", "true").expect("flag");
    flag_builder.set("opt_level", "speed").expect("flag");
    cranelift_native::builder()
        .expect("cranelift_native::builder")
        .finish(settings::Flags::new(flag_builder))
        .expect("isa finish")
}

#[test]
fn int_chunk_lowerer_emits_into_object_module() {
    // Build a JIT-capable Vm just for compile-front-end reuse — we
    // only need the embedded heap to interrogate compile + JIT
    // candidate Protos. The Vm itself never runs the produced .o.
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);

    // Lua chunk shape: a single arg-taking function whose body is a
    // pure int add. The int-chunk lowerer's whitelist accepts it.
    let _ = vm
        .eval(
            r#"
            function add(a, b)
                return a + b
            end
            return add(40, 2)
        "#,
        )
        .expect("eval");

    // Resolve the `add` function's Proto from the globals table.
    let key = vm.intern_str("add");
    let g = vm.globals();
    // SAFETY: `g` is a live `Gc<Table>` owned by the Vm; nothing else
    // holds a borrow into the table for the duration of this read.
    let add_val = unsafe { (*g.as_ptr()).get(luna_core::runtime::Value::Str(key)) };
    let cl = match add_val {
        luna_core::runtime::Value::Closure(c) => c,
        other => panic!("expected closure for `add`, got {other:?}"),
    };
    // SAFETY: `cl` is a live `Gc<LuaClosure>` returned from the live
    // globals table read above; no other borrow into the closure
    // exists for the duration of this field read.
    let proto = unsafe { (*cl.as_ptr()).proto };

    // Build an ObjectModule for the host triple. This is the deploy
    // shape — `finish() -> emit() -> Vec<u8>` produces an ELF / Mach-O
    // / PE `.o`.
    let isa = host_isa();
    let object_builder = ObjectBuilder::new(isa, "luna_aot_stage3_smoke", default_libcall_names())
        .expect("ObjectBuilder");
    let mut object_module = ObjectModule::new(object_builder);

    // Drive the SAME lowerer the runtime JIT uses, but against the
    // object module. This is the load-bearing assertion: the
    // generic signature `lower_int_chunk_into<M: Module>` accepts
    // `ObjectModule` exactly as it accepts `JITModule`.
    //
    // Note: a chunk that references `luna_jit_*` helper symbols
    // (NewTable / table_set_int / etc.) would need those symbols to
    // exist as `Linkage::Import` declarations resolvable at link
    // time. The `add` chunk above is pure int arith with no helper
    // calls, so the produced .o is self-contained — exactly the
    // shape Stage 4's pipeline expects for its smallest fixtures.
    let result = lower_int_chunk_into(&mut object_module, proto, false, false);

    let (fn_id, meta) = result.expect(
        "lower_int_chunk_into should accept the same int-chunk whitelist proto \
         the runtime JIT accepts, regardless of module backend",
    );

    // Sanity-check the meta: `add(a, b)` is a 2-arg Int returner.
    assert_eq!(meta.num_args, 2, "add takes two args");
    assert!(meta.returns_one, "add returns one value");
    assert!(!meta.ret_is_float, "add's return is Int, not Float");
    assert!(!meta.ret_is_table, "add's return is Int, not Table");

    // Force the linkage to be visible from outside the .o so a
    // future linker step has a symbol to look up. The lowerer
    // declared `luna_jit_chunk` as `Linkage::Local`; bump it to
    // `Export` for downstream visibility. (We use `declare_function`
    // with the SAME name + new linkage; cranelift_module merges.)
    let mut export_sig = object_module.make_signature();
    for _ in 0..meta.num_args {
        export_sig.params.push(cranelift_codegen::ir::AbiParam::new(
            cranelift_codegen::ir::types::I64,
        ));
    }
    export_sig
        .returns
        .push(cranelift_codegen::ir::AbiParam::new(
            cranelift_codegen::ir::types::I64,
        ));
    // No-op declare to confirm the FuncId is real and matches.
    let same_id = object_module
        .declare_function("luna_jit_chunk", Linkage::Local, &export_sig)
        .expect("re-declare same fn");
    assert_eq!(
        fn_id, same_id,
        "lowerer's FuncId must match the canonical fn name"
    );

    // Finish + emit. This is the deploy-side equivalent of
    // `JITModule::finalize_definitions` + `get_finalized_function`.
    let product = object_module.finish();
    let bytes = product.emit().expect("ObjectProduct::emit");

    assert!(
        !bytes.is_empty(),
        "emitted object file should contain bytes"
    );

    // Loose magic-byte check: ELF on Linux, Mach-O on macOS, PE on
    // Windows. Any of the three signals a valid object container.
    let is_elf = bytes.starts_with(&[0x7f, b'E', b'L', b'F']);
    let is_macho = bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
        || bytes.starts_with(&[0xfe, 0xed, 0xfa, 0xce]);
    let is_pe = bytes.starts_with(b"MZ");
    assert!(
        is_elf || is_macho || is_pe,
        "emitted bytes should be a recognized object format (got first 4 bytes: {:?})",
        &bytes[..bytes.len().min(4)]
    );
}
