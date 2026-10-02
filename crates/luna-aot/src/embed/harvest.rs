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

use luna_core::jit::trace_types::{CompileOptions, CompiledTrace, TraceRecord};
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;

use super::target::TargetSpec;
use super::trace_object::{build_object_module, emit_meta_sections, lower_and_encode_meta};
use super::{AotError, HarvestedTraces};

/// A captured trace that passed the AOT filter:
/// `(capture_index, head_proto_hash, head_pc, record, compiled_trace)`.
pub(super) type Installable = (
    usize,
    [u8; 16],
    u32,
    TraceRecord,
    luna_core::jit::send_compat::TArc<CompiledTrace>,
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

impl luna_core::jit::TraceCompiler for RecordingTraceCompiler {
    // `storage` is passed through to the inner backend.
    fn try_compile_trace(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        capture_record(record);
        self.inner.try_compile_trace(storage, record, opts)
    }

    fn try_compile_trace_for(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
        version: LuaVersion,
    ) -> Option<CompiledTrace> {
        capture_record(record);
        self.inner
            .try_compile_trace_for(storage, record, opts, version)
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
) -> Result<HarvestedTraces, AotError> {
    let Some(captured) = warmup_and_capture(dump_bytes, version) else {
        return Ok(HarvestedTraces::None);
    };

    let probe_on = std::env::var_os("LUNA_AOT_HARVEST_PROBE").is_some();
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

    emit_meta_sections(&mut module, &installable, blob_payload, &per_trace_meta)?;

    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| AotError::Object(format!("ObjectProduct::emit: {e}")))?;
    fs::write(out, &bytes)?;
    Ok(HarvestedTraces::Some)
}

/// Run the chunk once on a recording warmup `Vm` and return every
/// `TraceRecord` the dispatcher tried to compile. `None` when the dump
/// does not load.
fn warmup_and_capture(
    dump_bytes: &[u8],
    version: LuaVersion,
) -> Option<Vec<([u8; 16], u32, TraceRecord)>> {
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
    Some(AOT_CAPTURED_RECORDS.with(|cell| std::mem::take(&mut *cell.borrow_mut())))
}

/// Pair each captured record with the `CompiledTrace` that landed in
/// its proto and keep the AOT-installable ones.
fn select_installable(
    captured: Vec<([u8; 16], u32, TraceRecord)>,
    probe_on: bool,
) -> Vec<Installable> {
    // Pair each captured record with the actual CompiledTrace that
    // landed in the proto's traces vec — we need the post-lower fields
    // (window_size, entry_tags, exit_tags, dispatchable,
    // global_tag_res_kind) for the meta blob. The record-by-record
    // lookup keys on (proto_ptr, head_pc); records that didn't survive
    // compile (lowerer bailed, returned None) drop here.
    //
    // Filter to AOT-installable shapes. Wire format v2 carries
    // `per_exit_tags` (typed-register side-exit guards — GetUpval-
    // heavy traces). Wire format v3 carries `per_exit_inline`
    // (depth>0 inlined cmp side-exits).
    //
    // The wire format also doesn't ship sunk-alloc materialize
    // sites yet; `materialize_emit_count > 0` traces need that
    // path too — JIT-only.
    let mut installable: Vec<Installable> = Vec::new();
    let mut filter_stats = (0usize, 0usize, 0usize, 0usize);
    for (i, (hash, head_pc, record)) in captured.into_iter().enumerate() {
        let proto = record.head_proto;
        let traces_ref = proto.traces.borrow();
        let Some(ct) = traces_ref.iter().find(|c| c.head_pc == head_pc).cloned() else {
            filter_stats.0 += 1;
            continue;
        };
        drop(traces_ref);
        if !ct.dispatchable {
            filter_stats.1 += 1;
            continue;
        }
        if !ct.per_exit_inline.is_empty() {
            // depth>0 inlined cmp side-exits are supported. The lowerer's
            // `emit_chain_ptr_arg` routes the `FrameMaterializeInfo`
            // chain pointer through a relocatable data slot the
            // deploy-side `aot_inline_chain_resolver` populates at
            // startup; the v3 wire format's `per_exit_inline` tail
            // (cont_pc / head_resume_pc / packed exit_tags / packed
            // chain bytes) round-trips into a fresh
            // `Rc<[InlineSideExit]>` on the install side. The JIT path
            // bakes that chain pointer as a raw `iconst`, which under
            // AOT would be the warmup VM's heap address, invalid in the
            // deploy binary. Stat counter for diagnostics: how many of
            // the captured traces went through the v3 inline path.
            filter_stats.2 += 1;
        }
        // per_exit_tags is supported by wire format v2 — accept.
        // (Counter for diagnostics: how many of the captured traces
        // went through the v2 path.)
        if !ct.per_exit_tags.is_empty() {
            filter_stats.3 += 1;
        }
        installable.push((i, hash, head_pc, record, ct));
    }

    if probe_on {
        eprintln!(
            "luna-aot harvest: installable={}, filtered: no_ct={}, undispatchable={}, accepted_with_per_exit_inline={}, accepted_with_per_exit_tags={}",
            installable.len(),
            filter_stats.0,
            filter_stats.1,
            filter_stats.2,
            filter_stats.3,
        );
    }
    installable
}
