//! Deploy-side trace-meta walker.
//!
//! `luna-aot::embed::harvest_and_emit_aot_traces` emits a 48-byte
//! [`luna_core::jit::aot_meta::AotTraceIndexEntry`] per AOT-installable
//! trace into the `luna_trace_meta` bracketed section, plus a
//! combined meta-blob payload in `luna_trace_blob`. This walker
//! runs once at startup (between `vm.set_bytecode_loading(true)`
//! and `vm.load`), iterates the bracket-bounded section, matches
//! each entry's `proto_hash` against the loaded chunk's proto
//! tree via [`Vm::collect_proto_hashes`], and calls
//! [`Vm::install_aot_trace`] with a freshly constructed
//! [`CompiledTrace`] whose `entry` points at the linker-resolved
//! AOT mcode.
//!
//! Empty-section tolerant: a binary with zero linked trace `.o`s
//! has both bracket symbols collapse to the same address; the walk
//! short-circuits with `Ok(0)`.

use luna_core::jit::aot_meta::{
    AotTraceIndexEntry, decode_meta_blob, unpack_exit_tag, unpack_tag_res_kind,
};
use luna_core::jit::trace_types::{CompiledTrace, ExitTag, TraceFn};
use luna_core::vm::Vm;

// Bracket symbols — same pattern as the strkey_idx
// walker. ELF / lld auto-create `__start_<name>` / `__stop_<name>`
// for sections whose name is a valid C identifier; Mach-O uses
// `section$start$<seg>$<sect>` / `section$end$<seg>$<sect>`.
//
// Windows COFF has no bracket-symbol convention:
// the Windows path uses a runtime PE-header walk via
// [`crate::windows_section::find_section`] for the short-name
// section `.lt_meta` instead. See `windows_section` module docs.
#[cfg(all(unix, not(target_vendor = "apple")))]
unsafe extern "C" {
    #[link_name = "__start_luna_trace_meta"]
    static mut LUNA_TRACE_META_START: u8;
    #[link_name = "__stop_luna_trace_meta"]
    static mut LUNA_TRACE_META_END: u8;
}

#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    #[link_name = "\u{1}section$start$__DATA$luna_trace_meta"]
    static mut LUNA_TRACE_META_START: u8;
    #[link_name = "\u{1}section$end$__DATA$luna_trace_meta"]
    static mut LUNA_TRACE_META_END: u8;
}

/// Walk the `luna_trace_meta` section, install one `CompiledTrace`
/// per entry whose `proto_hash` matches a Proto reachable from
/// `root`. Returns the count installed.
///
/// Entries whose meta blob fails to decode (magic / version
/// mismatch, truncation) are skipped silently — the trace falls
/// back to JIT at runtime. `LUNA_AOT_PROBE=1` surfaces the count
/// + per-entry skip reasons on stderr for diagnosis.
///
/// The deploy `Vm` never side-traces an AOT-installed parent
/// (recorder is invoked from the dispatch path; AOT install
/// happens BEFORE the first dispatch), so the bare
/// [`CompiledTrace::from_aot_meta`] constructor with empty
/// `per_exit_inline` / `per_exit_tags` is sufficient.
pub fn install_all(
    vm: &mut Vm,
    root: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
) -> usize {
    // Locate the trace-meta section + length, dispatching on
    // target platform. Unix/Mach-O: linker-synthesised bracket
    // symbols. Windows: runtime PE-header walk via
    // [`crate::windows_section::find_section`]
    // for the short name `.lt_meta`. Either path can produce a
    // zero-length section (binary linked no AOT traces) —
    // `walk_meta_section` short-circuits.
    let (base, len_bytes): (*const u8, usize) = {
        #[cfg(target_os = "windows")]
        {
            match crate::windows_section::find_section(b".lt_meta") {
                Some((b, l)) => (b, l),
                None => return 0,
            }
        }
        #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
        {
            let start = &raw mut LUNA_TRACE_META_START as *mut AotTraceIndexEntry;
            let end = &raw mut LUNA_TRACE_META_END as *mut AotTraceIndexEntry;
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
        {
            let start = &raw mut LUNA_TRACE_META_START as *mut AotTraceIndexEntry;
            let end = &raw mut LUNA_TRACE_META_END as *mut AotTraceIndexEntry;
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        #[cfg(not(any(
            target_os = "windows",
            all(unix, not(target_vendor = "apple")),
            target_vendor = "apple"
        )))]
        {
            let _ = (vm, root);
            return 0;
        }
    };
    // SAFETY: `(base, len_bytes)` was produced either by a linker-
    // synthesised bracket symbol pair bounding a contiguous run of
    // `AotTraceIndexEntry`, or by [`windows_section::find_section`]
    // which returns the section's run-time base + virtual_size for
    // a PE section we ourselves emit via cranelift_object. Both
    // shapes satisfy `walk_meta_section`'s unsafe contract.
    unsafe { walk_meta_section(vm, root, base as *const AotTraceIndexEntry, len_bytes) }
}

/// Common walk shared by the Unix/Mach-O bracket-symbol path and
/// the Windows PE-header-located section path. Iterates one
/// [`AotTraceIndexEntry`] at a time, decoding the meta blob and
/// installing on the matched proto.
///
/// # Safety
///
/// `start` must point at the first byte of a `len_bytes`-long
/// run of `AotTraceIndexEntry` instances, all in readable memory
/// (linker-defined section or PE-mapped section data). Per-entry
/// `meta_ptr` / `fn_ptr` are validated by the per-entry null
/// checks; meta blob bytes are bounded by the entry's `meta_len`.
unsafe fn walk_meta_section(
    vm: &mut Vm,
    root: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
    start: *const AotTraceIndexEntry,
    len_bytes: usize,
) -> usize {
    if start.is_null() || len_bytes < core::mem::size_of::<AotTraceIndexEntry>() {
        return 0;
    }
    let n_entries = len_bytes / core::mem::size_of::<AotTraceIndexEntry>();
    let proto_hashes = vm.collect_proto_hashes(root);
    let probe_on = std::env::var_os("LUNA_AOT_PROBE").is_some();
    let mut installed = 0usize;
    // SAFETY: caller invariant — [start, start+n_entries) is
    // mapped readable memory containing valid AotTraceIndexEntry
    // instances or zero-fill placeholder bytes (skipped via the
    // fn_ptr == 0 guard).
    unsafe {
        for i in 0..n_entries {
            let entry = &*start.add(i);
            // The placeholder entry in `luna_trace_meta` is a single
            // zero byte from the cmain shim — the section walk steps
            // past it via the size-rounding above. An entry whose
            // `fn_ptr` is null came from that placeholder and must
            // be skipped (not from a real AOT-emitted trace).
            if entry.fn_ptr == 0 || entry.meta_ptr == 0 {
                continue;
            }
            let meta_bytes =
                core::slice::from_raw_parts(entry.meta_ptr as *const u8, entry.meta_len as usize);
            let decoded = match decode_meta_blob(meta_bytes) {
                Ok(d) => d,
                Err(reason) => {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_trace skip head_pc={} reason={reason}",
                            entry.head_pc
                        );
                    }
                    continue;
                }
            };
            // Find the matching Proto by hash.
            let matched = proto_hashes
                .iter()
                .find(|(_p, h)| *h == entry.proto_hash)
                .map(|(p, _h)| *p);
            let Some(proto) = matched else {
                if probe_on {
                    eprintln!(
                        "luna-runtime-helpers: aot_trace skip head_pc={} reason=proto_hash_unmatched",
                        entry.head_pc
                    );
                }
                continue;
            };
            // Reconstruct exit_tags + entry_tags + global_tag_res_kind.
            let mut exit_tags_vec: Vec<ExitTag> = Vec::with_capacity(decoded.exit_tags.len());
            let mut tag_decode_ok = true;
            for raw in decoded.exit_tags.iter().copied() {
                if let Some(t) = unpack_exit_tag(raw) {
                    exit_tags_vec.push(t);
                } else {
                    tag_decode_ok = false;
                    break;
                }
            }
            let Some(tag_res_kind) = unpack_tag_res_kind(decoded.header.tag_res_kind) else {
                if probe_on {
                    eprintln!(
                        "luna-runtime-helpers: aot_trace skip head_pc={} reason=tag_res_kind_invalid",
                        entry.head_pc
                    );
                }
                continue;
            };
            if !tag_decode_ok {
                if probe_on {
                    eprintln!(
                        "luna-runtime-helpers: aot_trace skip head_pc={} reason=exit_tag_invalid",
                        entry.head_pc
                    );
                }
                continue;
            }
            let entry_tags_rc: luna_core::jit::send_compat::TArc<[u8]> = decoded.entry_tags.into();
            let exit_tags_rc: luna_core::jit::send_compat::TArc<[ExitTag]> = exit_tags_vec.into();
            // v2 per_exit_tags decode: reconstruct
            // `Vec<(cont_pc, Rc<[ExitTag]>)>` matching the
            // dispatcher's `decode_exit_shape` shape lookup. Each
            // entry's packed-byte `ExitTag` array unpacks via
            // [`unpack_exit_tag`]; an invalid byte = skip the
            // whole trace (matches the existing exit-tag handling).
            let mut per_exit_tags_decoded: Vec<(
                u32,
                luna_core::jit::send_compat::TArc<[ExitTag]>,
            )> = Vec::with_capacity(decoded.per_exit_tags.len());
            let mut per_exit_tags_ok = true;
            for ent in &decoded.per_exit_tags {
                let mut tags: Vec<ExitTag> = Vec::with_capacity(ent.tags_packed.len());
                for raw in ent.tags_packed.iter().copied() {
                    if let Some(t) = unpack_exit_tag(raw) {
                        tags.push(t);
                    } else {
                        per_exit_tags_ok = false;
                        break;
                    }
                }
                if !per_exit_tags_ok {
                    break;
                }
                per_exit_tags_decoded.push((ent.cont_pc, tags.into()));
            }
            if !per_exit_tags_ok {
                if probe_on {
                    eprintln!(
                        "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_tag_invalid",
                        entry.head_pc
                    );
                }
                continue;
            }
            // The per_exit_inline decode is load-bearing. Each wire entry's
            // `chain_bytes` rebuilds into a fresh
            // `Vec<FrameMaterializeInfo>` → `Rc<[...]>` for the
            // dispatcher's `per_exit_inline[i].chain` field; the
            // `tags_packed` array unpacks through `unpack_exit_tag`
            // into `Rc<[ExitTag]>` (same shape as the v2
            // per_exit_tags pattern above). A failing chain rebuild
            // (length not a multiple of 12 — corruption) or an
            // invalid packed tag (out-of-range byte) means the
            // trace skips install; the trace then falls back to
            // JIT at runtime via the recorder.
            //
            // The IR-baked chain pointer lives in a separate slot
            // populated by `aot_inline_chain_resolver::resolve_all`
            // (called from `run_inner` BEFORE this install path).
            // The two chain owners (this Rc and the leaked Rc
            // behind the slot) are independent allocations of the
            // same byte content — neither side compares pointers.
            let Some(per_exit_inline_decoded) =
                decode_per_exit_inline(&decoded.per_exit_inline, entry.head_pc, probe_on)
            else {
                continue;
            };
            // Transmute the C-ABI fn ptr from u64 (wire-width-
            // stable) to `TraceFn`. Safe because the trace .o was
            // emitted by `lower_trace_into_named` with sig
            // `(I64) -> I64`, matching `TraceFn`. AOT-binary
            // deploy is always 64-bit so the u64 narrows to a
            // valid pointer on this target.
            let fn_ptr_raw = entry.fn_ptr as *const u8;
            let trace_entry: TraceFn = core::mem::transmute::<*const u8, TraceFn>(fn_ptr_raw);
            let ct = CompiledTrace::from_aot_meta(
                trace_entry,
                decoded.header.head_pc,
                decoded.header.n_ops,
                decoded.header.dispatchable != 0,
                decoded.header.window_size,
                entry_tags_rc,
                exit_tags_rc,
                tag_res_kind,
                per_exit_tags_decoded,
                per_exit_inline_decoded,
            );
            vm.install_aot_trace(proto, ct);
            installed += 1;
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_trace_installed head_pc={}",
                    decoded.header.head_pc
                );
            }
        }
    }
    installed
}

/// Rebuild the v3 `per_exit_inline` side exits of one trace. `None`
/// (after printing the probe line) when a chain or a packed tag is
/// corrupt, in which case the whole trace skips install.
fn decode_per_exit_inline(
    entries: &[luna_core::jit::aot_meta::PerExitInlineEntry],
    head_pc: u32,
    probe_on: bool,
) -> Option<Vec<luna_core::jit::trace_types::InlineSideExit>> {
    let mut per_exit_inline_decoded: Vec<luna_core::jit::trace_types::InlineSideExit> =
        Vec::with_capacity(entries.len());
    let mut inline_ok = true;
    for ent in entries {
        let Some(chain_vec) = ent.rebuild_chain() else {
            inline_ok = false;
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_chain_invalid (cont_pc={})",
                    head_pc, ent.cont_pc
                );
            }
            break;
        };
        let mut tags: Vec<ExitTag> = Vec::with_capacity(ent.tags_packed.len());
        for raw in ent.tags_packed.iter().copied() {
            if let Some(t) = unpack_exit_tag(raw) {
                tags.push(t);
            } else {
                inline_ok = false;
                break;
            }
        }
        if !inline_ok {
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_tag_invalid (cont_pc={})",
                    head_pc, ent.cont_pc
                );
            }
            break;
        }
        per_exit_inline_decoded.push(luna_core::jit::trace_types::InlineSideExit {
            cont_pc: ent.cont_pc,
            head_resume_pc: ent.head_resume_pc,
            exit_tags: tags.into(),
            chain: chain_vec.into(),
            side_trace_ptr: Box::new(luna_core::jit::send_compat::TCellPtr::null()),
        });
    }
    if !inline_ok {
        return None;
    }
    Some(per_exit_inline_decoded)
}
