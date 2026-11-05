//! Emitting harvested traces into the trace `.o`: the lowered mcode, the
//! combined `luna_trace_blob` payload and one `luna_trace_meta` index
//! entry per trace.

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};

use luna_core::__private::CompileOptions;
use luna_core::jit::aot_meta::{
    AotTraceMetaHeader, PerExitInlineEntry, PerExitTagsEntry, encode_meta_blob, pack_exit_tag,
    pack_tag_res_kind,
};
use luna_core::version::LuaVersion;

use super::AotError;
use super::harvest::Installable;
use super::target::TargetSpec;

/// Build the ObjectModule for the trace .o. PIC required for ELF/
/// Mach-O linker relocations.
///
/// `cranelift_isa_builder()` resolves the per-target ISA (host =
/// `cranelift_native` for CPU-feature detection; cross =
/// `isa::lookup` over the parsed triple). For cross targets we get
/// a `TargetIsa` whose codegen matches the deploy ABI rather than
/// the build host's.
pub(super) fn build_object_module(target: &TargetSpec) -> Result<ObjectModule, AotError> {
    let isa = {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .expect("flag");
        flag_builder.set("is_pic", "true").expect("flag");
        flag_builder.set("opt_level", "speed").expect("flag");
        target
            .cranelift_isa_builder()?
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| {
                AotError::Object(format!(
                    "Cranelift ISA finish for target {}: {e}",
                    target.triple
                ))
            })?
    };
    let object_builder = ObjectBuilder::new(isa, "luna_aot_traces", default_libcall_names())
        .map_err(|e| AotError::Object(format!("ObjectBuilder: {e}")))?;
    Ok(ObjectModule::new(object_builder))
}

/// Per-trace emission: lower IR + meta blob.
///
/// One lowered trace's entry in the meta index.
pub(super) struct TraceMeta {
    fn_name: String,
    hash: [u8; 16],
    head_pc: u32,
    blob_offset: u32,
    blob_len: u32,
}

/// Returns the combined blob payload and one [`TraceMeta`] per lowered
/// trace. A trace whose AOT lowering bails is left out, so the entries
/// carry their own hash and head pc rather than an index into `installable`.
pub(super) fn lower_and_encode_meta(
    module: &mut ObjectModule,
    installable: &[Installable],
    version: LuaVersion,
    probe_on: bool,
) -> (Vec<u8>, Vec<TraceMeta>) {
    // We accumulate the meta blobs into one combined `luna_trace_blob`
    // data symbol (so the linker has a single object per pipeline,
    // not N), and emit one `luna_trace_meta_<idx>` entry per trace.
    let mut blob_payload: Vec<u8> = Vec::new();
    let mut per_trace_meta: Vec<TraceMeta> = Vec::new();
    for (idx, hash, head_pc, record, ct) in installable.iter() {
        let fn_name = format!("luna_aot_trace_{idx:08x}");
        let opts = CompileOptions {
            internal_loop: true,
            // the same dialect flag the JIT compiled these records with
            pre53: version <= LuaVersion::Lua53,
            aot: true,
            tier: Default::default(),
            tier_up_at: 0,
            dialect: Some(version),
        };
        // Re-lower this record into the ObjectModule under a unique
        // exported name. Any bail here = the record was lowerable at
        // warmup time (it appeared in the captured list AND the
        // matched ct is non-None) but failed under aot=true codegen —
        // most likely a relocation path that fails for some opcode the
        // AOT lowerer's strkey resolver doesn't cover. Skip + continue;
        // the trace will fall back to JIT at deploy time.
        let lower_res = luna_jit::jit_backend::trace::lower_trace_into_named_for(
            module,
            record,
            opts,
            Some(&fn_name),
            version,
        );
        if lower_res.is_none() {
            if probe_on {
                eprintln!(
                    "luna-aot harvest: AOT lower bailed for trace idx={idx} head_pc={}",
                    ct.head_pc()
                );
            }
            continue;
        }

        // Serialize the meta blob for this trace.
        let entry_tags_vec: Vec<u8> = ct.entry_tags().to_vec();
        let exit_tags_vec: Vec<u8> = ct.exit_tags().iter().copied().map(pack_exit_tag).collect();
        // v2 tail — per-cont_pc typed-register side-exit shapes.
        // Each entry's `tags_packed` mirrors the JIT-time
        // `(cont_pc, tags)` exit, packed through
        // `pack_exit_tag` so the deploy-side reader unpacks via the
        // same byte → ExitTag mapping.
        let per_exit_tags_entries: Vec<PerExitTagsEntry> = ct
            .per_exit_tags()
            .iter()
            .map(|(cont_pc, tags)| PerExitTagsEntry {
                cont_pc,
                tags_packed: tags.iter().copied().map(pack_exit_tag).collect(),
            })
            .collect();
        let header = AotTraceMetaHeader {
            magic: luna_core::jit::aot_meta::AOT_META_MAGIC,
            version: luna_core::jit::aot_meta::AOT_META_VERSION,
            head_pc: ct.head_pc(),
            n_ops: ct.n_ops(),
            window_size: ct.window_size(),
            dispatchable: u8::from(ct.dispatchable()),
            tag_res_kind: pack_tag_res_kind(ct.global_tag_res_kind()),
            entry_tags_len: entry_tags_vec.len() as u16,
            exit_tags_len: exit_tags_vec.len() as u32,
        };
        // Populate the v3 inline tail from the live trace's
        // `per_exit_inline`. Each `InlineSideExit` round-trips into a
        // `PerExitInlineEntry` via the byte-stable converter: tags pack
        // through `pack_exit_tag` and the chain serialises as raw
        // `repr(C)` bytes (12 per record). The deploy install path decodes
        // the entries back into fresh `Rc<[FrameMaterializeInfo]>` /
        // `Rc<[ExitTag]>` allocations whose contents match what the
        // JIT-time recorder produced; the IR's chain pointer is fed
        // from a separate `aot_inline_chain_resolver`-populated
        // slot (no shared ownership with the dispatcher field —
        // neither side compares pointers).
        let per_exit_inline_entries: Vec<PerExitInlineEntry> = ct
            .per_exit_inline()
            .iter()
            .map(PerExitInlineEntry::from_inline_side_exit)
            .collect();
        let blob = encode_meta_blob(
            &header,
            &entry_tags_vec,
            &exit_tags_vec,
            &per_exit_tags_entries,
            &per_exit_inline_entries,
        );
        let blob_offset = blob_payload.len() as u32;
        let blob_len = blob.len() as u32;
        blob_payload.extend_from_slice(&blob);
        per_trace_meta.push(TraceMeta {
            fn_name,
            hash: *hash,
            head_pc: *head_pc,
            blob_offset,
            blob_len,
        });
    }
    (blob_payload, per_trace_meta)
}

mod sections;
pub(super) use sections::emit_meta_sections;

#[cfg(test)]
mod tests;
