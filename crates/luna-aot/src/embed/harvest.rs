//! Offline trace harvester: a warmup run that records every trace the
//! JIT compiles, then hands the AOT-installable ones to
//! [`super::trace_object`] for emission.
//
// The pipeline:
//   1. Build a JIT-equipped warmup `Vm` (luna_jit::new_with_jit) and
//      swap in a `RecordingTraceCompiler` wrapper that forwards every
//      compile attempt to the real Cranelift backend AND captures the
//      input `TraceRecord` into a thread-local. The recorder runs at
//      every back-edge that crosses the const `TRACE_HOT_THRESHOLD = 64`,
//      so simple counted loops with iteration counts in the thousands
//      will close at least one trace.
//   2. Load the dump and call the chunk's root closure. The dispatcher
//      records + compiles + dispatches as normal; we only care about
//      the captured records.
//   3. For each captured (proto, record) pair, re-lower the record via
//      `lower_trace_into_named` against a fresh `ObjectModule` per .o,
//      writing the emitted bytes + a per-trace `luna_trace_blob` payload
//      + a `luna_trace_meta` 48-byte index entry. All three sections are
//      bracket-symbol enumerated by the deploy walker at startup.
//   4. Bail-tolerantly: if no traces close (small / non-loopy source),
//      return `HarvestedTraces::None` and the pipeline skips the trace .o.

use std::fs;
use std::path::Path;

use luna_core::__private::{CompileOptions, CompiledTrace, TraceRecord};
use luna_core::jit::aot_meta::AotRole;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

use super::target::TargetSpec;
use super::trace_object::{build_object_module, emit_meta_sections, lower_and_encode_meta};
use super::{AotError, HarvestedTraces};

/// A captured trace that passed the AOT filter:
/// `(capture_index, head_proto_hash, head_pc, record, compiled_trace, role)`.
pub(super) type Installable = (
    usize,
    [u8; 16],
    u32,
    TraceRecord,
    luna_core::jit::send_compat::TArc<CompiledTrace>,
    AotRole,
);

/// Records every `TraceRecord` the dispatcher tries to compile; forwards
/// the actual compile to the wrapped real Cranelift backend so the
/// warmup run dispatches normally afterwards.
///
/// Records are appended to a thread-local Vec — read out after the
/// warmup `vm.call_value` returns.
struct RecordingTraceCompiler {
    inner: luna_jit::jit_backend::CraneliftBackend,
}

thread_local! {
    /// Thread-local capture buffer for `TraceRecord`s observed during
    /// the AOT warmup run. Each entry is `(head_proto_hash,
    /// head_pc, TraceRecord)`. Cleared at the start of every
    /// [`harvest_and_emit_aot_traces`] call.
    static AOT_CAPTURED_RECORDS: std::cell::RefCell<Vec<([u8; 16], u32, TraceRecord)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Capture a cloneable image of the record so we can re-lower at AOT
/// emit time. TraceRecord is Clone since every field is Clone
/// (Gc<Proto> = NonNull copy; Vec<RecordedOp> is Clone).
fn capture_record(record: &TraceRecord) {
    let hash = record.head_proto.stable_hash();
    AOT_CAPTURED_RECORDS.with(|cell| {
        cell.borrow_mut()
            .push((hash, record.head_pc, record.clone()));
    });
}

// SAFETY: what it returns comes unchanged from the inner backend, which meets the
// conditions itself
unsafe impl luna_core::__private::TraceCompiler for RecordingTraceCompiler {
    // `storage` is passed through to the inner backend.
    fn try_compile_trace(
        &self,
        storage: &mut dyn luna_core::__private::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        let ct = self.inner.try_compile_trace(storage, record, opts);
        if ct.is_some() {
            capture_record(record);
        }
        ct
    }

    fn try_compile_trace_for(
        &self,
        storage: &mut dyn luna_core::__private::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
        version: LuaVersion,
    ) -> Option<CompiledTrace> {
        // only what compiled: a recording that failed has no trace to pair
        let ct = self
            .inner
            .try_compile_trace_for(storage, record, opts, version);
        if ct.is_some() {
            capture_record(record);
        }
        ct
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        self.inner.last_compile_checkpoint()
    }
}

/// Run a warmup `Vm` on the dumped bytecode, harvest closed
/// `TraceRecord`s the trace JIT compiled, re-lower each through
/// `lower_trace_into_named` into a fresh `ObjectModule`, and write the
/// produced bytes (plus a `luna_trace_meta` index + `luna_trace_blob`
/// payload) to `out`.
///
/// Returns `Ok(HarvestedTraces::None)` if the warmup recorded zero
/// AOT-installable traces — the calling pipeline then skips the .o on
/// the link line entirely (no placeholder .o on disk).
///
/// `dump_bytes` is the same chunk the deploy binary will execute, so
/// the proto identities (and therefore `stable_hash`) match between
/// AOT compile and deploy load.
pub(super) fn harvest_and_emit_aot_traces(
    dump_bytes: &[u8],
    version: LuaVersion,
    out: &Path,
    target: &TargetSpec,
    probe_on: bool,
) -> Result<HarvestedTraces, AotError> {
    // the captured records point into the warmup vm's heap (head_proto and
    // the traces hanging off it), so the vm must outlive every use of them
    let Some((_warmup_vm, captured)) = warmup_and_capture(dump_bytes, version) else {
        return Ok(HarvestedTraces::None);
    };

    if probe_on {
        eprintln!("luna-aot harvest: captured {} TraceRecords", captured.len());
    }
    if captured.is_empty() {
        return Ok(HarvestedTraces::None);
    }

    let installable = select_installable(captured, probe_on);
    if installable.is_empty() {
        return Ok(HarvestedTraces::None);
    }

    let mut module = build_object_module(target)?;
    let (blob_payload, per_trace_meta) =
        lower_and_encode_meta(&mut module, &installable, version, probe_on);

    if per_trace_meta.is_empty() {
        // Everything filtered out by the AOT lower bail. Same shape as
        // "no traces at all".
        return Ok(HarvestedTraces::None);
    }

    emit_meta_sections(&mut module, blob_payload, &per_trace_meta)?;

    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| AotError::Object(format!("ObjectProduct::emit: {e}")))?;
    fs::write(out, &bytes)?;
    Ok(HarvestedTraces::Some)
}

/// Run the chunk once on a recording warmup `Vm` and return every
/// `TraceRecord` the dispatcher tried to compile. `None` when the dump
/// does not load. The `Vm` is returned with the records because they hold
/// raw pointers into its heap.
fn warmup_and_capture(
    dump_bytes: &[u8],
    version: LuaVersion,
) -> Option<(Vm, Vec<([u8; 16], u32, TraceRecord)>)> {
    // Reset the capture buffer for this harvest call. A previous run
    // in the same process (e.g. unit tests running back-to-back) must
    // not leak its records into this one.
    AOT_CAPTURED_RECORDS.with(|cell| cell.borrow_mut().clear());

    // Build the warmup Vm + install the recording compiler. We use
    // `new_with_jit` (= new_minimal_with_jit + open_all_libs) so the
    // script can call `print`, `math.*`, etc. — typical hot loops in
    // realistic programs touch these libs.
    let mut vm = luna_jit::new_with_jit(version);
    vm.install_jit_backend(
        luna_jit::jit_backend::CraneliftBackend,
        RecordingTraceCompiler {
            inner: luna_jit::jit_backend::CraneliftBackend,
        },
    );
    vm.set_trace_jit_enabled(true);
    // Chunk JIT short-circuits recursive `Op::Call` at exec.rs:1567 before
    // push_frame, hiding the helper body from the trace recorder and
    // suppressing the inline side-exit chain. Trace JIT
    // subsumes chunk JIT's coverage and adds the inline-side-exit support
    // chunk JIT lacks entirely, so harvest skips chunk JIT.
    vm.set_jit_enabled(false);
    vm.set_bytecode_loading(true);

    // Load + call the root closure. Errors here are **non-fatal** —
    // the script may rely on host-side state we don't have (CLI args,
    // env vars), may `error()` intentionally as part of normal control
    // flow, or may diverge from any reproducible path. The warmup's
    // job is to surface hot traces, not to validate the script —
    // produce 0 AOT traces and move on if anything goes sideways.
    //
    // The deploy binary still runs the script through interp + JIT;
    // a missing AOT fast-path doesn't break correctness, only perf.
    let closure = match vm.load(dump_bytes, b"=embedded-aot-warmup") {
        Ok(c) => c,
        Err(_) => return None,
    };
    let _ = vm.call_value(Value::Closure(closure), &[]);

    // Snapshot the captured records. Take ownership so the thread-
    // local Vec is empty going forward (matches the test-isolation
    // invariant established at the top of this fn).
    let captured = AOT_CAPTURED_RECORDS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    Some((vm, captured))
}

mod select;
use select::select_installable;

#[cfg(test)]
mod strkey_section_tests;

#[cfg(test)]
mod relink_tests;

#[cfg(test)]
mod entry_side_tests;
