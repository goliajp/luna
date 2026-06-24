//! v1.3 Phase AOT Stage 7 sub-piece 2 — load-bearing smoke for
//! iconst-baked-address relocation in the trace lowerer.
//!
//! # What sub-piece 2 changes
//!
//! Pre-sub-piece-2 the trace lowerer (`crates/luna-jit/src/jit_backend/
//! trace.rs`, four iconst sites listed in `.dev/rfcs/v1.3-rfc-trace-
//! aot-relocation.md`) baked the recorder-side `Gc<LuaStr>::as_ptr()`
//! as `iconst(I64, <addr>)` directly into the IR. That's correct for
//! the JIT path (`M = JITModule`, lowered mcode runs in-process with
//! the recorder `Vm`) but garbage for the AOT path — the deploy
//! binary's `StringTable` lives at a different address and contains
//! a different `LuaStr` for the same UTF-8 bytes.
//!
//! Sub-piece 2 adds `CompileOptions { aot: true }`. When set, the
//! lowerer's `emit_str_key_arg` helper routes each interned-string
//! key through two stably-named data objects (deduped via cranelift's
//! `Module::declare_data` name interning):
//!
//! - `__luna_aot_strkey_slot_<hex>` — 8-byte writable slot, IR loads
//!   the runtime `Gc<LuaStr>::as_ptr()` through it. Zero-initialised
//!   at link time; the deploy-side resolver writes the resolved
//!   pointer at startup before any AOT trace dispatches.
//! - `__luna_aot_strkey_bytes_<hex>` — read-only, layout `[u64 len ||
//!   bytes...]`. The deploy resolver walks every `_bytes_*` symbol,
//!   interns the bytes into its own `Vm.heap`, and stores the
//!   resulting pointer in the matching `_slot_*`.
//!
//! `<hex>` is a 16-char FNV-1a-64 prefix over the UTF-8 bytes. Pure-
//! Rust hash, no `sha2` dep (luna-core 0-third-party-dep contract).
//!
//! # What this test asserts
//!
//! 1. The lowerer accepts `opts.aot = true` against an `ObjectModule`
//!    and produces a non-empty `.o`.
//! 2. The emitted `.o` symbol table contains BOTH the writable slot
//!    and the read-only bytes object for the test trace's key string.
//! 3. The runtime JIT path with `opts.aot = false` is byte-for-byte
//!    unchanged (covered by the existing `trace_jit_s11_step_a` suite;
//!    we don't re-run it here, just record the invariant).
//!
//! # What this test does NOT prove
//!
//! - The deploy-side runtime resolver (sub-piece 3 of the RFC; lives
//!   in `luna-runtime-helpers`). Without that, an AOT binary that
//!   links a sub-piece-2 `.o` would load through an all-zero slot
//!   and segfault on first dispatch. That's the next session's work.
//! - End-to-end "AOT binary fires AOT mcode on a hot loop". Requires
//!   sub-pieces 3 (slot resolver) + 4 (trace registry / dispatch
//!   install) and is the charter AOT acceptance gate.
//! - The four `state[0] = t.as_ptr() as i64` sites at trace.rs:
//!   8240/8264/8282/8314 — those are TEST CODE that constructs trace
//!   input state, not lowerer IR. They are not relocation targets;
//!   per RFC § "Gc<Table> deopt-payload sites" we recommend the AOT
//!   pipeline emits traces with `MAX_GUARD_FAILS = 0` so deopt
//!   payloads aren't an AOT concern at all (v1.4 follow-up).
//! - The interned string `key_str.as_ptr()` cast from `LuaStr` to
//!   `*const u8` produces a pointer with the *same* numeric value as
//!   the AOT-time process address — i.e., we *could* hash that
//!   number for `<hex>`. We hash the bytes instead so the deploy
//!   resolver can reproduce the hash from `_bytes_*` contents
//!   without needing to ship a mapping side-channel.

use std::process::Command;

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};

use luna_core::jit::trace_types::{CompileOptions, RecordedOp, TraceRecord};
use luna_core::runtime::Gc;
use luna_core::runtime::function::Proto;
use luna_core::version::LuaVersion;
use luna_core::vm::isa::{Inst, Op};
use luna_jit::jit_backend::trace::lower_trace_into;

/// PIC-on TargetIsa so the `.o` can be relocated by a linker. JIT
/// uses `is_pic = false` (mcode pinned at known mmap addresses); AOT
/// MUST flip it on so ELF / Mach-O relocations work.
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

/// Resolve a proto for a Lua snippet that puts the string `"field"`
/// into its const pool at index 0 — we'll point `Op::SetField`'s
/// K[B] at index 0 in the synthetic trace below.
fn load_proto_with_field_const(vm: &mut luna_core::vm::Vm) -> Gc<Proto> {
    // The body `t.field = v` plus a return forces `field` into
    // `consts[0]` and gives us a proto with max_stack ≥ 2.
    let _ = vm
        .eval(
            r#"
            function setter(t, v)
                t.field = v
                return t
            end
            return setter
        "#,
        )
        .expect("eval");
    let key = vm.intern_str("setter");
    let g = vm.globals();
    // SAFETY: `g` is the live globals `Gc<Table>`; nothing else
    // borrows for this read.
    let v = unsafe { (*g.as_ptr()).get(luna_core::runtime::Value::Str(key)) };
    let cl = match v {
        luna_core::runtime::Value::Closure(c) => c,
        other => panic!("expected closure for `setter`, got {other:?}"),
    };
    // SAFETY: `cl` live `Gc<LuaClosure>` from the read above.
    let proto = unsafe { (*cl.as_ptr()).proto };
    // Locate index of "field" in const pool — the synthetic trace
    // below addresses it by index.
    let field_idx = proto
        .consts
        .iter()
        .position(|c| matches!(c, luna_core::runtime::Value::Str(s) if s.as_bytes() == b"field"))
        .expect("proto must intern `field` in consts");
    assert_eq!(
        field_idx, 0,
        "test assumes `field` is consts[0]; got {field_idx}. \
         Lua source change broke the fixture — relocate the const \
         lookup or recompile expectation."
    );
    proto
}

/// One-op trace: `R[0][K[0]:string] := R[1]` — drives the SetField
/// helper-path branch at trace.rs:5897 which calls our new
/// `emit_str_key_arg`.
fn make_setfield_record(proto: Gc<Proto>) -> TraceRecord {
    let mut rec = TraceRecord::start(proto, 0, Vec::new(), false);
    let pushed = rec.push(RecordedOp {
        proto,
        pc: 0,
        // Op::SetField R[A=0][K[B=0]:string] := R[C=1].
        // K=true flag set so the lowerer takes the K[B] const path.
        inst: Inst::iabc(Op::SetField, 0, 0, 1, true),
        inline_depth: 0,
        var_count: None,
    });
    assert!(pushed, "synthetic 1-op record must fit MAX_TRACE_LEN");
    rec.closed = true;
    rec
}

/// FNV-1a-64 16-hex of "field" — must match
/// `crates/luna-jit/src/jit_backend/trace.rs::strkey_hex_label` byte
/// for byte. If the lowerer's hash changes, this test fails noisily
/// rather than silently producing the wrong symbol.
fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn have_on_path(prog: &str) -> bool {
    Command::new(prog)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn setfield_trace_aot_emits_strkey_data_symbols() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let proto = load_proto_with_field_const(&mut vm);
    let record = make_setfield_record(proto);

    let isa = host_pic_isa();
    let object_builder = ObjectBuilder::new(isa, "luna_aot_sp2_smoke", default_libcall_names())
        .expect("ObjectBuilder");
    let mut object_module = ObjectModule::new(object_builder);

    let opts = CompileOptions {
        internal_loop: false,
        pre53: false,
        aot: true,
    };

    // The lowerer's `dispatchable` analysis may reject this 1-op
    // SetField record (no exit, no return path). We don't care
    // about runtime correctness here — we care that IF the lowerer
    // emits ANY code for a SetField op, the strkey data symbols
    // come along. Try the full lower; if it bails, skip with a
    // diagnostic so the test still surfaces signal.
    let Some((fn_id, _compiled)) = lower_trace_into(&mut object_module, &record, opts) else {
        panic!(
            "lower_trace_into bailed on 1-op SetField record — \
             synthetic shape may have drifted from lowerer \
             acceptance. Inspect with RUST_LOG and add the missing \
             accepted-shape op."
        );
    };
    let _ = fn_id;

    let product = object_module.finish();
    let bytes = product.emit().expect("ObjectProduct::emit");
    assert!(!bytes.is_empty(), "AOT lower must emit a non-empty .o file");

    // Persist the bytes to a tempfile so `nm` can read them.
    let dir = tempfile::tempdir().expect("tempdir");
    let obj_path = dir.path().join("sp2_smoke.o");
    std::fs::write(&obj_path, &bytes).expect("write .o");

    if !have_on_path("nm") {
        eprintln!(
            "stage7_iconst_reloc: `nm` not on PATH — symbol check \
             skipped, but the .o was produced ({} bytes). Install \
             binutils / Xcode CLT to enable the full assertion.",
            bytes.len()
        );
        return;
    }

    let nm_out = Command::new("nm")
        .arg(&obj_path)
        .output()
        .expect("nm spawn");
    assert!(
        nm_out.status.success(),
        "nm failed: {}",
        String::from_utf8_lossy(&nm_out.stderr)
    );
    let nm_str = String::from_utf8_lossy(&nm_out.stdout);

    let hex = fnv1a64_hex(b"field");
    // Mach-O nm prepends `_` to symbol names; ELF nm does not.
    // Match the bare core name to stay portable.
    let slot_core = format!("luna_aot_strkey_slot_{hex}");
    let bytes_core = format!("luna_aot_strkey_bytes_{hex}");
    let idx_core = format!("luna_aot_strkey_idx_{hex}");

    assert!(
        nm_str.contains(&slot_core),
        "emitted .o must contain `__{slot_core}` (writable slot). \
         nm output:\n{nm_str}"
    );
    assert!(
        nm_str.contains(&bytes_core),
        "emitted .o must contain `__{bytes_core}` (bytes manifest). \
         nm output:\n{nm_str}"
    );

    // Sub-piece 3 — index entry per unique key. Local linkage, so
    // `nm` reports it lowercase-`t`/`s` (small letter = local). The
    // deploy-side resolver doesn't need it by name (it walks the
    // dedicated section instead) but its presence is the load-bearing
    // contract for the resolver's whole-program walk.
    assert!(
        nm_str.contains(&idx_core),
        "emitted .o must contain `__{idx_core}` (index entry; \
         sub-piece 3 contract). nm output:\n{nm_str}"
    );

    // Section walk: verify the dedicated `luna_strkey_idx` section is
    // present. The deploy-side resolver brackets this section
    // whole-program via `__start_luna_strkey_idx` (ELF) /
    // `section$start$__DATA$luna_strkey_idx` (Mach-O), so the
    // section name on the .o must match exactly. Use `objdump -h` on
    // ELF, `otool -s __DATA luna_strkey_idx` on Mach-O.
    let (probe_cmd, probe_args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("otool", vec!["-l"])
    } else {
        ("objdump", vec!["-h"])
    };
    if have_on_path(probe_cmd) {
        let probe = Command::new(probe_cmd)
            .args(&probe_args)
            .arg(&obj_path)
            .output();
        if let Ok(out) = probe {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                text.contains("luna_strkey_idx"),
                "`{probe_cmd}` did not surface section `luna_strkey_idx` on \
                 sp2 .o (sub-piece 3 expects it as the resolver walk target). \
                 {probe_cmd} output:\n{text}"
            );
        }
    }
}
