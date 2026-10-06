//! `FrameMaterializeInfo` chain pointer reloc resolver. See the comment below for the full
//! design rationale; this module owns the deploy-side walk +
//! per-chain Rc materialization + slot write.

// Deploy-side inline-chain resolver.
//
// Mirrors `aot_strkey_resolver`'s shape. The trace lowerer's
// `emit_chain_ptr_arg` (`crates/luna-jit/src/jit_backend/trace.rs`)
// emits one `(slot, bytes, idx)` triple per unique
// `FrameMaterializeInfo` chain when `opts.aot == true`; this resolver
// walks the bracketed `luna_inline_chnx` section (Unix / Mach-O) or the
// PE-header-located `.lt_chai` section (Windows), parses each bytes
// payload into a `Vec<FrameMaterializeInfo>`, leaks it as a
// process-lifetime `Rc<[...]>` (so the IR's load yields a valid pointer
// for the binary's lifetime — there is no per-trace tear-down on the
// AOT path), and writes the chain's first-element pointer into the
// matching slot.
//
// Why a separate Rc instead of pointing the slot at the bytes section
// directly:
//   - The IR loads the slot, then passes the value as `*const
//     FrameMaterializeInfo` to `luna_jit_trace_materialize_frames`,
//     which interprets the bytes as a fully-aligned array of `repr(C)`
//     structs. The bytes section is already 8-byte aligned with the
//     same packing, so a `bytes_ptr + 8` (skip the count prefix) would
//     work, but a future change to `FrameMaterializeInfo` layout would
//     silently misinterpret stale bytes. Going through an explicit
//     `Vec → Rc<[...]>` conversion lets us validate `chain_bytes.len()
//     % 16 == 0` (via the same `PerExitInlineEntry::FRAME_MATERIALIZE
//     _INFO_SIZE` constant the v3 wire format uses) and surface
//     corruption with a probe message rather than dispatching into
//     garbage.
//   - Keeping the chain's ownership on the Rust side mirrors the JIT
//     path's `per_exit_inline_vec.push((..., chain_rc, ...))` — the
//     dispatcher's `CompiledTrace::per_exit_inline[i].chain` field can
//     hold its own Rc rebuilt from the same bytes (decoded by the
//     trace install path); the IR pointer and the dispatcher field
//     point at independently-allocated copies of the same data, but
//     neither side compares pointers, only reads through them.
use luna_core::jit::trace_types::FrameMaterializeInfo;

/// Wire-size of one `FrameMaterializeInfo` record on disk and in
/// the bytes section payload. Asserted at compile time in
/// `luna_core::jit::aot_meta::FRAME_MATERIALIZE_INFO_WIRE_SIZE_CHECK`.
const FRAME_MATERIALIZE_INFO_SIZE: usize = 16;

/// Index entry layout — must match the cranelift-emit shape in
/// `crates/luna-jit/src/jit_backend/trace.rs::emit_chain_ptr_arg`:
/// two pointer-sized fields, `bytes_ptr` and `slot_ptr`, both
/// resolved by the static linker before process load completes.
#[repr(C)]
struct IndexEntry {
    /// Address of the `__luna_aot_inline_chain_bytes_<hex>` symbol:
    /// `[u64 count | packed_records...]` payload, read-only. The
    /// records are tightly packed 16-byte
    /// `(base_offset, pc, nresults)` triples.
    bytes_ptr: *const u8,
    /// Address of the `__luna_aot_inline_chain_slot_<hex>` symbol:
    /// writable 8-byte slot, zero-initialised at link time. This
    /// resolver writes the leaked chain's first-element pointer
    /// here.
    slot_ptr: *mut *const FrameMaterializeInfo,
}

// ELF / lld auto-creates `__start_<name>` / `__stop_<name>` for
// sections whose name is a valid C identifier (`luna_inline_chnx`).
// Mach-O uses `section$start$<seg>$<sect>` /
// `section$end$<seg>$<sect>`, synthesised by Apple `ld`. Windows
// COFF has neither — see the runtime PE-header walker in the
// `resolve_all` arm.
// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(all(unix, not(target_vendor = "apple")))]
unsafe extern "C" {
    #[link_name = "__start_luna_inline_chnx"]
    static mut LUNA_INLINE_CHNX_START: u8;
    #[link_name = "__stop_luna_inline_chnx"]
    static mut LUNA_INLINE_CHNX_END: u8;
}

// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    #[link_name = "\u{1}section$start$__DATA$luna_inline_chnx"]
    static mut LUNA_INLINE_CHNX_START: u8;
    #[link_name = "\u{1}section$end$__DATA$luna_inline_chnx"]
    static mut LUNA_INLINE_CHNX_END: u8;
}

/// Walk the bracketed `luna_inline_chnx` section (Unix / Mach-O) or
/// the PE-header-located `.lt_chai` section (Windows). For each
/// entry: rebuild a `Vec<FrameMaterializeInfo>` from the bytes
/// payload, materialise as a `Rc<[...]>`, leak ownership (process-
/// lifetime — AOT traces never tear down), write the chain's
/// first-element pointer into the matching slot.
///
/// Returns the number of slots populated (zero on a binary that
/// linked zero AOT traces with inline cmp@d>0 side-exits).
///
/// Tolerant of trailing-zero placeholder entries (the cmain shim
/// emits one zero-filled IndexEntry to guarantee the section exists
/// even when no real chain symbols are linked) via the null-pointer
/// guard in `walk_index_bytes`.
pub fn resolve_all() -> usize {
    let (base, len_bytes): (*const u8, usize) = {
        #[cfg(target_os = "windows")]
        {
            match crate::windows_section::find_section(b".lt_chai") {
                Some((b, l)) => (b, l),
                None => return 0,
            }
        }
        #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
        {
            let start = &raw mut LUNA_INLINE_CHNX_START as *mut IndexEntry;
            let end = &raw mut LUNA_INLINE_CHNX_END as *mut IndexEntry;
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
        {
            let start = &raw mut LUNA_INLINE_CHNX_START as *mut IndexEntry;
            let end = &raw mut LUNA_INLINE_CHNX_END as *mut IndexEntry;
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
            return 0;
        }
    };
    // SAFETY: `(base, len_bytes)` bounds the `luna_inline_chnx` section
    // (`.lt_chai` on Windows): the linker's start/stop symbols, or the
    // section the PE header names, and `luna-aot` fills that section
    // only with `IndexEntry`s laid out as `walk_index_bytes` reads them
    unsafe { walk_index_bytes(base, len_bytes) }
}

/// Common per-entry walk shared by the Unix/Mach-O bracket-symbol
/// path and the Windows PE-header-located section path.
///
/// Each entry's `bytes_ptr` points at `[u64 count, records...]`;
/// we decode the count, validate `count * 16` doesn't overflow,
/// parse `count` `FrameMaterializeInfo` triples, materialise them
/// as an `Rc<[FrameMaterializeInfo]>`, leak ownership via
/// `core::mem::forget(rc.clone())` (the inner buffer stays alive
/// for process lifetime), and write the first-element pointer into
/// the slot. Subsequent AOT mcode dispatches read the slot and pass
/// the pointer to `luna_jit_trace_materialize_frames(n, ptr)`.
///
/// Corrupt entries (null pointers, unaligned count, count overflow)
/// are skipped silently with an `LUNA_AOT_PROBE` line on stderr —
/// the trace's first inline side-exit dispatch will then deopt via
/// the helper's `pending_err` path because the slot stays NULL.
///
/// # Safety
/// `base` is null or points at `len_bytes` readable bytes holding
/// `IndexEntry`s, aligned for them, as `luna-aot` emits the section:
/// each entry's non-null `bytes_ptr` points at a count followed by that
/// many records, and its non-null `slot_ptr` at a writable pointer slot.
unsafe fn walk_index_bytes(base: *const u8, len_bytes: usize) -> usize {
    if base.is_null() || len_bytes == 0 {
        return 0;
    }
    let probe_on = std::env::var_os("LUNA_AOT_PROBE").is_some();
    let n_entries = len_bytes / core::mem::size_of::<IndexEntry>();
    let start = base as *const IndexEntry;
    let mut populated = 0usize;
    // SAFETY: `base` points at `len_bytes` readable bytes of aligned
    // `IndexEntry`s (# Safety), and `n_entries` of them fit there
    let entries = unsafe { core::slice::from_raw_parts(start, n_entries) };
    for (i, entry) in entries.iter().enumerate() {
        if entry.bytes_ptr.is_null() || entry.slot_ptr.is_null() {
            continue;
        }
        // Bytes layout: little-endian u64 record count, then
        // `count * FRAME_MATERIALIZE_INFO_SIZE` packed bytes.
        // SAFETY: a non-null `bytes_ptr` starts with an eight-byte count
        // (# Safety)
        let count = unsafe { core::ptr::read_unaligned(entry.bytes_ptr as *const u64) } as usize;
        let Some(bytes_len) = count.checked_mul(FRAME_MATERIALIZE_INFO_SIZE) else {
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_inline_chain skip entry {i} reason=count_overflow count={count}"
                );
            }
            continue;
        };
        // SAFETY: the count is followed by `count` records of
        // `FRAME_MATERIALIZE_INFO_SIZE` bytes (# Safety), `bytes_len` in all
        let raw = unsafe { core::slice::from_raw_parts(entry.bytes_ptr.add(8), bytes_len) };
        let mut vec: Vec<FrameMaterializeInfo> = Vec::with_capacity(count);
        for j in 0..count {
            let off = j * FRAME_MATERIALIZE_INFO_SIZE;
            let base_offset = u32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
            let pc = u32::from_le_bytes(raw[off + 4..off + 8].try_into().unwrap());
            let nresults = i32::from_le_bytes(raw[off + 8..off + 12].try_into().unwrap());
            let n_varargs = u32::from_le_bytes(raw[off + 12..off + 16].try_into().unwrap());
            vec.push(FrameMaterializeInfo {
                base_offset,
                pc,
                nresults,
                n_varargs,
            });
        }
        let rc: luna_core::jit::send_compat::TArc<[FrameMaterializeInfo]> = vec.into();
        // `Rc<[T]>::as_ptr` returns a fat `*const [T]`; the
        // first-element address is what the IR's
        // `luna_jit_trace_materialize_frames` consumes. For a
        // non-empty chain, `rc[0]` is the data pointer; for an
        // empty chain (count == 0) the IR never reaches the
        // helper (the side-exit's `if !call_chain.is_empty()`
        // gate at compile time would have routed through the
        // d=0 arm), so the slot stays at a dangling-but-unused
        // value. Guard anyway for paranoia.
        let chain_ptr: *const FrameMaterializeInfo = if count == 0 {
            core::ptr::null()
        } else {
            &rc[0] as *const FrameMaterializeInfo
        };
        // Leak ownership so the chain bytes stay alive for the
        // process. AOT-installed traces never tear down (no
        // `proto.traces.borrow_mut().remove(...)` path on
        // deploy), so a single leak per unique chain matches
        // the lifetime requirement exactly.
        core::mem::forget(rc);
        // SAFETY: a non-null `slot_ptr` points at a writable pointer slot
        // (# Safety)
        unsafe { core::ptr::write(entry.slot_ptr, chain_ptr) };
        populated += 1;
    }
    populated
}
