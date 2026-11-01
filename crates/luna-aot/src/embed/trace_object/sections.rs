//! The trace `.o`'s data: the combined meta blob and the index entries.

use cranelift_module::{DataDescription, Linkage, Module};
use cranelift_object::ObjectModule;

use super::super::AotError;
use super::TraceMeta;

/// Emit the combined `luna_trace_blob` data symbol and one meta index
/// entry per lowered trace.
pub(in crate::embed) fn emit_meta_sections(
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
