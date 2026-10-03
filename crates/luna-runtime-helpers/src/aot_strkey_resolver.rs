//! Deploy-side interned-string slot resolver.
//!
//! AOT trace mcode emitted by [`luna_jit::jit_backend::trace::
//! lower_trace_into`] with `CompileOptions { aot: true }` reads
//! interned-string-key pointers indirectly through writable 8-byte
//! slots (`__luna_aot_strkey_slot_<hex>`). Each unique key
//! contributes a 16-byte `[bytes_addr, slot_addr]` entry to a
//! dedicated `luna_strkey_idx` section. The deploy binary's static
//! linker auto-brackets that section via
//! `__start_luna_strkey_idx` / `__stop_luna_strkey_idx` (ELF) or
//! `section$start$__DATA$luna_strkey_idx` /
//! `section$end$__DATA$luna_strkey_idx` (Mach-O), and this resolver
//! walks the bracketed range to: (a) intern each bytes block into
//! the deploy `Vm`'s heap, and (b) write the resulting
//! `Gc<LuaStr>::as_ptr()` into the matching slot.
//!
//! # Safety contract (called from `run_inner` only)
//!
//! - Must run **once**, BEFORE the deploy `Vm` dispatches into any
//!   AOT mcode. `run_inner` calls it after `Vm::new` /
//!   `set_bytecode_loading` and before `vm.load`.
//! - Idempotent under second-call: the slots already hold valid
//!   `Gc<LuaStr>` pointers, the section walk re-interns the bytes
//!   (cheap — string-table dedup), and re-writes the slot with the
//!   same pointer.
//! - Empty-section tolerant: a deploy binary that linked zero AOT
//!   trace `.o`s has both bracket symbols collapsing to the same
//!   address; the walk loop terminates with zero entries.
//!
//! # Why feature-gated on `jit-helpers`
//!
//! The whole AOT-trace path is jit-helper-gated. With
//! `default-features = false` the staticlib excludes
//! `luna-jit` and the resolver becomes a no-op — interp-only AOT
//! binaries pay zero scan cost and the bracket-symbol references are
//! elided.
use luna_core::vm::Vm;

/// Index entry layout — must match the cranelift-emit shape in
/// `crates/luna-jit/src/jit_backend/trace.rs::emit_str_key_arg`:
/// two pointer-sized fields, `bytes_ptr` and `slot_ptr`, both
/// resolved by the static linker before process load completes.
#[repr(C)]
struct IndexEntry {
    /// Address of the `__luna_aot_strkey_bytes_<hex>` symbol:
    /// `[u64 len | utf8...]` payload, read-only.
    bytes_ptr: *const u8,
    /// Address of the `__luna_aot_strkey_slot_<hex>` symbol:
    /// writable 8-byte slot, zero-initialised at link time, this
    /// resolver writes the interned `Gc<LuaStr>` pointer in.
    slot_ptr: *mut *const u8,
}

// ELF / lld auto-creates `__start_<name>` / `__stop_<name>` for
// sections whose name is a valid C identifier. Our section is
// `luna_strkey_idx` (set via cranelift's `set_segment_section`).
//
// Mach-O uses a different convention: `section$start$<seg>$<sect>`
// / `section$end$<seg>$<sect>`, synthesized by Apple `ld`. We
// declare per-platform externs and the dead-strip pass discards
// whichever doesn't match.
//
// Windows / COFF has no bracket-symbol convention:
// `link.exe` / `lld-link` don't synthesize `__start_` / `__stop_`
// externs. Instead the deploy walker calls into the parent crate's
// [`crate::windows_section::find_section`] which does a runtime
// PE-header parse via `GetModuleHandleW(NULL)`. The Windows path
// uses the short section name `.lt_skix` (8 bytes, COFF
// section-name max) — see `crates/luna-aot/src/embed.rs` for the
// emit-side choice. Empty-section (no AOT traces linked in) is
// handled by `find_section` returning `None` and `resolve_all`
// short-circuiting to 0.
// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(all(unix, not(target_vendor = "apple")))]
unsafe extern "C" {
    #[link_name = "__start_luna_strkey_idx"]
    static mut LUNA_STRKEY_IDX_START: u8;
    #[link_name = "__stop_luna_strkey_idx"]
    static mut LUNA_STRKEY_IDX_END: u8;
}

// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    #[link_name = "\u{1}section$start$__DATA$luna_strkey_idx"]
    static mut LUNA_STRKEY_IDX_START: u8;
    #[link_name = "\u{1}section$end$__DATA$luna_strkey_idx"]
    static mut LUNA_STRKEY_IDX_END: u8;
}

/// Walk the bracketed `luna_strkey_idx` section (Unix / Mach-O) or
/// the PE-header-located `.lt_skix` section (Windows), intern each
/// bytes block into `vm.heap`, write the resulting pointer into
/// the matching slot. Returns the number of slots populated
/// (zero on a binary that linked zero AOT trace `.o`s).
pub fn resolve_all(vm: &mut Vm) -> usize {
    // Locate the strkey-idx section + length, dispatching on
    // target platform. Windows: runtime PE header walk via
    // [`crate::windows_section::find_section`] for the short name
    // `.lt_skix` (mirrors the emit-side choice in
    // `crates/luna-aot/src/embed.rs::write_aot_cmain_object_for`
    // Windows arm + the harvester's `set_segment_section`).
    // Unix/Mach-O: bracket symbols supplied by the linker. Either
    // dispatch path can produce a zero-length section (binary
    // linked no AOT traces) — `walk_index_bytes` short-circuits.
    let (base, len_bytes): (*const u8, usize) = {
        #[cfg(target_os = "windows")]
        {
            match crate::windows_section::find_section(b".lt_skix") {
                Some((b, l)) => (b, l),
                None => return 0,
            }
        }
        #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
        {
            let start = &raw mut LUNA_STRKEY_IDX_START as *mut IndexEntry;
            let end = &raw mut LUNA_STRKEY_IDX_END as *mut IndexEntry;
            // start == end on a binary with zero AOT traces. Section
            // length = end - start in bytes; divide by entry size
            // gives the count.
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
        {
            let start = &raw mut LUNA_STRKEY_IDX_START as *mut IndexEntry;
            let end = &raw mut LUNA_STRKEY_IDX_END as *mut IndexEntry;
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        // Platforms without a section enumeration path (e.g.
        // wasm32) — no AOT install possible, return 0.
        #[cfg(not(any(
            target_os = "windows",
            all(unix, not(target_vendor = "apple")),
            target_vendor = "apple"
        )))]
        {
            let _ = vm;
            return 0;
        }
    };
    // SAFETY: `(base, len_bytes)` bounds the `luna_strkey_idx` section
    // (`.lt_skix` on Windows): the linker's start/stop symbols, or the
    // section the PE header names, and `luna-aot` fills that section
    // only with `IndexEntry`s laid out as `walk_index_bytes` reads them
    unsafe { walk_index_bytes(vm, base, len_bytes) }
}

/// Common per-entry walk shared by the Unix/Mach-O bracket-symbol
/// path and the Windows PE-header-located section path. Takes the
/// section base + length in bytes (as the two enumeration paths
/// produce different types — a pair of bracket symbol addresses on
/// Unix, a `(*const u8, usize)` tuple from [`crate::windows_section
/// ::find_section`] on Windows) and walks `len / sizeof(IndexEntry)`
/// entries.
///
/// Tolerant of trailing-zero placeholder entries (the cmain shim
/// emits one zero-filled IndexEntry to guarantee the section exists
/// even when no real traces are linked in) via the
/// `entry.bytes_ptr.is_null() || entry.slot_ptr.is_null()` skip.
///
/// # Safety
/// `base` is null or points at `len_bytes` readable bytes holding
/// `IndexEntry`s, aligned for them, as `luna-aot` emits the section:
/// each entry's non-null `bytes_ptr` points at a length followed by that
/// many bytes, and its non-null `slot_ptr` at a writable pointer slot.
unsafe fn walk_index_bytes(vm: &mut Vm, base: *const u8, len_bytes: usize) -> usize {
    if base.is_null() || len_bytes == 0 {
        return 0;
    }
    let n_entries = len_bytes / core::mem::size_of::<IndexEntry>();
    let start = base as *const IndexEntry;
    let mut populated = 0usize;
    // SAFETY: `base` points at `len_bytes` readable bytes of aligned
    // `IndexEntry`s (# Safety), and `n_entries` of them fit there
    let entries = unsafe { core::slice::from_raw_parts(start, n_entries) };
    for entry in entries {
        if entry.bytes_ptr.is_null() || entry.slot_ptr.is_null() {
            continue;
        }
        // SAFETY: a non-null `bytes_ptr` points at an eight-byte length and
        // then that many bytes (# Safety)
        let bytes = unsafe {
            let len = core::ptr::read_unaligned(entry.bytes_ptr as *const u64) as usize;
            core::slice::from_raw_parts(entry.bytes_ptr.add(8), len)
        };
        let interned = vm.heap.intern(bytes);
        // SAFETY: a non-null `slot_ptr` points at a writable pointer slot
        // (# Safety)
        unsafe { core::ptr::write(entry.slot_ptr, interned.as_ptr() as *const u8) };
        populated += 1;
    }
    populated
}
