//! Emitting harvested traces into the trace `.o`: the lowered mcode, the
//! combined `luna_trace_blob` payload and one `luna_trace_meta` index
//! entry per trace.

use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{DataDescription, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};

use luna_core::jit::aot_meta::{
    AotTraceMetaHeader, PerExitInlineEntry, PerExitTagsEntry, encode_meta_blob, pack_exit_tag,
    pack_tag_res_kind,
};
use luna_core::jit::trace_types::CompileOptions;
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
                    ct.head_pc
                );
            }
            continue;
        }

        // Serialize the meta blob for this trace.
        let entry_tags_vec: Vec<u8> = ct.entry_tags.iter().copied().collect();
        let exit_tags_vec: Vec<u8> = ct.exit_tags.iter().copied().map(pack_exit_tag).collect();
        // v2 tail — per-cont_pc typed-register side-exit shapes.
        // Each entry's `tags_packed` mirrors the JIT-time
        // `(cont_pc, Rc<[ExitTag]>)` pair, packed through
        // `pack_exit_tag` so the deploy-side reader unpacks via the
        // same byte → ExitTag mapping.
        let per_exit_tags_entries: Vec<PerExitTagsEntry> = ct
            .per_exit_tags
            .iter()
            .map(|(cont_pc, tags)| PerExitTagsEntry {
                cont_pc: *cont_pc,
                tags_packed: tags.iter().copied().map(pack_exit_tag).collect(),
            })
            .collect();
        let header = AotTraceMetaHeader {
            magic: luna_core::jit::aot_meta::AOT_META_MAGIC,
            version: luna_core::jit::aot_meta::AOT_META_VERSION,
            head_pc: ct.head_pc,
            n_ops: ct.n_ops,
            window_size: ct.window_size,
            dispatchable: u8::from(ct.dispatchable),
            tag_res_kind: pack_tag_res_kind(ct.global_tag_res_kind),
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
            .per_exit_inline
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

/// Emit the combined `luna_trace_blob` data symbol and one meta index
/// entry per lowered trace.
pub(super) fn emit_meta_sections(
    module: &mut ObjectModule,
    blob_payload: Vec<u8>,
    per_trace_meta: &[TraceMeta],
) -> Result<(), AotError> {
    // Emit the combined `luna_trace_blob` data symbol. Single object;
    // each per-trace meta entry references it via offset.
    let blob_data_id = module
        .declare_data("__luna_trace_blob_combined", Linkage::Local, false, false)
        .map_err(|e| AotError::Object(format!("declare_data blob: {e}")))?;
    {
        let mut desc = DataDescription::new();
        desc.define(blob_payload.into_boxed_slice());
        // Read-only data section. Mach-O caps section names at 16 chars;
        // `luna_trace_blob` is 15 — fits with one byte of headroom for
        // the implicit comparator NUL.
        // `__DATA` segment on Mach-O — the deploy walker doesn't
        // bracket this section (each meta entry references it by
        // pointer relocation), but keeping it next to `luna_trace_
        // meta` in `__DATA` is the path of least surprise for ld /
        // strip.
        //
        // PE section names are capped at 8 bytes in the final image;
        // COFF uses `.lt_blob` (7 chars + leading `.`) so the post-link
        // PE preserves the name byte-for-byte. The deploy walker
        // doesn't bracket-look this section (it's only referenced via
        // pointer relocations from `.lt_meta` entries), so the name
        // choice is mostly for consistency / debuggability.
        desc.set_custom_section(&aot_data_section(
            module.isa().triple(),
            "luna_trace_blob",
            ".lt_blob",
        ));
        module
            .define_data(blob_data_id, &desc)
            .map_err(|e| AotError::Object(format!("define_data blob: {e}")))?;
    }

    // Emit one 48-byte `luna_trace_meta_<idx>` index entry per trace
    // into the dedicated `luna_trace_meta` section. The static linker
    // auto-brackets via `__start_luna_trace_meta` / `__stop_luna_trace_
    // meta` (ELF) or `section$start$__DATA$luna_trace_meta` (Mach-O).
    for (idx, meta) in per_trace_meta.iter().enumerate() {
        let TraceMeta {
            fn_name,
            hash,
            head_pc,
            blob_offset,
            blob_len,
        } = meta;

        let entry_data_id = module
            .declare_data(
                &format!("__luna_trace_meta_entry_{idx:08x}"),
                Linkage::Local,
                false,
                false,
            )
            .map_err(|e| AotError::Object(format!("declare_data meta entry: {e}")))?;
        let mut desc = DataDescription::new();
        // 48 bytes: [hash 16] [head_pc 4] [_pad 4] [fn_ptr 8] [meta_ptr 8] [meta_len 4] [_pad2 4]
        let mut payload = [0u8; 48];
        payload[0..16].copy_from_slice(hash);
        payload[16..20].copy_from_slice(&head_pc.to_le_bytes());
        // payload[20..24] _pad
        // payload[24..32] fn_ptr — relocation
        // payload[32..40] meta_ptr — relocation
        payload[40..44].copy_from_slice(&blob_len.to_le_bytes());
        desc.define(Box::new(payload));
        // On Mach-O the section must be in `__DATA` so it merges with
        // the cmain shim's `__DATA,luna_trace_meta` placeholder; in any
        // other segment the deploy walker's
        // `section$start$__DATA$luna_trace_meta` sees only the
        // placeholder.
        //
        // COFF: short name `.lt_meta` matches the cmain shim's
        // placeholder section and the deploy walker's
        // [`windows_section::find_section`] needle. PE section names
        // are capped at 8 bytes in the final linked image —
        // `luna_trace_meta` (15 chars) would either truncate
        // unpredictably or land in the COFF string table that the
        // linker drops.
        desc.set_custom_section(&aot_data_section(
            module.isa().triple(),
            "luna_trace_meta",
            ".lt_meta",
        ));
        // 8-byte alignment for the fn_ptr / meta_ptr relocations at
        // offsets 24 and 32. Without explicit `set_align(8)` the
        // entries can land at odd offsets in the .o, and Mach-O's
        // `ld` rejects the unaligned pointer slots ("pointer not
        // aligned in `___luna_trace_meta_entry_…`+0x20").
        desc.set_align(8);

        // Declare the trace fn as Import so we can relocate against it.
        // (It's Export'd by `lower_trace_into_named` above.) The
        // declare_function call here re-uses the same name; cranelift
        // module name-interning matches the two so the relocation
        // resolves to the lowerer-emitted body.
        let trace_fn_sig = {
            let mut sig = module.make_signature();
            sig.params.push(cranelift_codegen::ir::AbiParam::new(
                cranelift_codegen::ir::types::I64,
            ));
            sig.returns.push(cranelift_codegen::ir::AbiParam::new(
                cranelift_codegen::ir::types::I64,
            ));
            sig
        };
        let fn_id = module
            .declare_function(fn_name, Linkage::Import, &trace_fn_sig)
            .map_err(|e| AotError::Object(format!("declare_function for reloc: {e}")))?;
        let fn_gv = module.declare_func_in_data(fn_id, &mut desc);
        desc.write_function_addr(24, fn_gv);
        let blob_gv = module.declare_data_in_data(blob_data_id, &mut desc);
        desc.write_data_addr(32, blob_gv, *blob_offset as i64);

        module
            .define_data(entry_data_id, &desc)
            .map_err(|e| AotError::Object(format!("define_data meta entry: {e}")))?;
    }
    Ok(())
}

/// Custom section name for trace data on the target's object format.
/// Mach-O takes `segment,section`; COFF gets the 8-byte name because
/// the final PE keeps only short names; ELF takes the name as is.
fn aot_data_section(triple: &target_lexicon::Triple, name: &str, coff_name: &str) -> String {
    match triple.binary_format {
        target_lexicon::BinaryFormat::Macho => format!("__DATA,{name}"),
        target_lexicon::BinaryFormat::Coff => coff_name.to_owned(),
        _ => name.to_owned(),
    }
}

#[cfg(test)]
mod tests;
