//! v1.3 Phase AOT Stage 7 polish 3 — deploy-side PE/COFF section
//! walker for Windows AOT binaries.
//!
//! # Why this module exists
//!
//! On Unix targets (ELF + lld) and macOS (Mach-O + Apple ld), the
//! static linker auto-synthesizes `__start_<name>` / `__stop_<name>`
//! (ELF) or `section$start$<seg>$<sect>` / `section$end$<seg>$<sect>`
//! (Mach-O) bracket symbols around any user-defined section whose
//! name is a valid C identifier. The deploy walkers in
//! `aot_strkey_resolver` and `aot_trace_registry` rely on those
//! externs to find their bracketed sections at run time.
//!
//! **Windows PE/COFF has no such convention.** Neither `link.exe` nor
//! `lld-link` synthesize bracket symbols. The two paths to enumerate
//! a section at run time on Windows are:
//!
//! 1. **Per-section registry**: emit a per-`.o` "(slot_id, bytes_id)"
//!    pair into a known-name registry section, with one bracket
//!    written by hand into a shim object. Strip-friendly but requires
//!    changes to the lowerer's data-emission shape.
//!
//! 2. **PE header walk**: at run time, call `GetModuleHandleW(NULL)`
//!    to get the loaded module's image base, parse the DOS stub /
//!    NT headers / section table, and find the named section by
//!    walking the `IMAGE_SECTION_HEADER[]` array. Zero compile-time
//!    cost, zero changes to the emit path — the walker is purely a
//!    deploy-side concern.
//!
//! Option (2) is what we implement here. It's the conventional shape
//! for Windows-side section enumeration (e.g. `libunwind`'s
//! `__unw_init_local`, the Rust stdlib's TLS callback registry).
//!
//! # PE section name 8-byte limit
//!
//! PE/COFF section headers carry the name as a fixed `[u8; 8]` field
//! (`IMAGE_SECTION_HEADER::Name`). Object-file format COFF supports
//! long names via a `"/<decimal-offset>"` reference into the COFF
//! string table — but that mechanism is **not preserved** by either
//! `link.exe` or `lld-link` into the final PE image. Any input
//! section name > 8 bytes lands in the PE with either a truncated
//! name (typical) or a `/<offset>` form pointing at a string table
//! the linker chose not to emit (rare; produces an unfindable
//! section).
//!
//! Consequence: the `luna-aot` Windows path uses **deliberately
//! short** section names (`.lt_meta` for trace meta, `.lt_skix` for
//! strkey idx, both 7 chars) so the post-link PE preserves the name
//! verbatim and our walker can match by exact byte equality.
//!
//! # Why hand-rolled winapi externs
//!
//! `luna-runtime-helpers` already depends on `luna-jit` (Stage 7
//! sub-piece 1) which transitively pulls cranelift + a Windows winapi
//! crate. We could use those types, but the surface this module needs
//! is tiny (one fn import + one struct definition), and hand-rolling
//! keeps the dependency story for non-Windows targets unchanged. The
//! `winapi` / `windows-sys` ecosystem also routinely shifts shape
//! between major versions; a hand-rolled local extern is immune to
//! upstream churn.
//!
//! # Safety contract
//!
//! - Only safe to call **once the binary is fully loaded**. In the
//!   AOT-binary deploy shape, `luna_aot_run` is invoked from the C
//!   `main`, which runs after the loader has mapped the entire image
//!   and applied base relocations — the contract is trivially met.
//! - `GetModuleHandleW(NULL)` returns the image base of the calling
//!   process. The PE header layout is stable across Windows versions
//!   (PE32+ has been the only 64-bit shape since Vista).
//! - All pointer arithmetic stays within the image bounds verified
//!   by [`find_section`]'s `e_lfanew` + `NumberOfSections` checks.

#![allow(non_camel_case_types, non_snake_case)]

// ────────────────────────────────────────────────────────────────────
// Hand-rolled winapi externs.
//
// Only one fn import: `GetModuleHandleW(NULL)` returns the image base
// of the current process as `HMODULE` (= `*mut u8`). Linked against
// kernel32, which is already in the `luna-aot` link line for the
// MinGW Windows target (see `crates/luna-aot/src/embed.rs::link_aot_
// binary_for` Windows arm: `-lkernel32`).
//
// `#[link(name = "kernel32")]` is redundant on `x86_64-pc-windows-gnu`
// (MinGW's link line carries `-lkernel32` by default and rust's
// `windows-targets` ships an import lib), but stating it explicitly
// avoids "undefined reference to `GetModuleHandleW`" if a future
// MinGW config drops the default lib.
// ────────────────────────────────────────────────────────────────────

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(lpModuleName: *const u16) -> *mut u8;
}

// ────────────────────────────────────────────────────────────────────
// PE/COFF on-disk header layout (PE32+, x86_64 / arm64).
//
// The structs below are subsets — we only define fields up to the
// last one we read. The layout is `#[repr(C, packed)]` because PE
// headers are byte-streams without natural alignment; the
// `read_unaligned` calls in `find_section` are the safe way to
// dereference these.
// ────────────────────────────────────────────────────────────────────

/// DOS stub header at the start of every PE image. The only field
/// we care about is `e_lfanew` — the offset to the NT headers.
#[repr(C, packed)]
struct ImageDosHeader {
    /// "MZ" magic — verified by [`find_section`] before we trust the
    /// rest of the header. PE images always start with this stub.
    e_magic: u16,
    _reserved: [u16; 29],
    /// File offset to the `IMAGE_NT_HEADERS64` structure.
    e_lfanew: i32,
}

/// COFF file header — sits at `image_base + e_lfanew + 4` (after the
/// "PE\0\0" signature). Carries the section count.
#[repr(C, packed)]
struct ImageFileHeader {
    machine: u16,
    /// Number of `IMAGE_SECTION_HEADER` entries that follow the
    /// optional header. The walker iterates this many entries.
    number_of_sections: u16,
    time_date_stamp: u32,
    pointer_to_symbol_table: u32,
    number_of_symbols: u32,
    /// Size in bytes of the optional header that follows this
    /// structure. For PE32+ this is typically `0xF0` (240). The
    /// section table starts at `<this_header_offset + size_of::<
    /// ImageFileHeader>() + size_of_optional_header>`.
    size_of_optional_header: u16,
    characteristics: u16,
}

/// PE section header — fixed 40 bytes per entry. We only need
/// `name`, `virtual_size`, and `virtual_address` to compute the
/// section's in-memory base + length. `virtual_address` is the RVA
/// (offset from `image_base`); the run-time base is `image_base +
/// virtual_address` after the loader has applied base relocations.
#[repr(C, packed)]
struct ImageSectionHeader {
    /// 8-byte section name, NUL-padded. Names ≤ 7 chars carry their
    /// trailing NUL; 8-char names fill the whole array (and aren't
    /// NUL-terminated — match by byte slice, not C string).
    name: [u8; 8],
    /// Size of the section's actual data. For initialized data this
    /// equals `size_of_raw_data` when the section isn't padded.
    virtual_size: u32,
    /// RVA from `image_base`. Add to `image_base` to get the
    /// run-time address of the section's first byte.
    virtual_address: u32,
    size_of_raw_data: u32,
    pointer_to_raw_data: u32,
    pointer_to_relocations: u32,
    pointer_to_linenumbers: u32,
    number_of_relocations: u16,
    number_of_linenumbers: u16,
    characteristics: u32,
}

// ────────────────────────────────────────────────────────────────────
// Public API.
// ────────────────────────────────────────────────────────────────────

/// Find a PE section by name in the calling process's loaded image.
///
/// Returns `Some((ptr, len))` where `ptr` is the section's run-time
/// start address and `len` is `virtual_size` (the section's logical
/// length, not the file-padded `size_of_raw_data`). Returns `None`
/// when:
///
/// - The PE header doesn't parse (DOS magic mismatch, `e_lfanew`
///   out of plausible range — defensive checks against a stripped
///   or corrupted binary).
/// - The requested section name isn't present.
/// - The section is present but has `virtual_size == 0` (empty —
///   treated as "not found" so the caller's bracket-walk equivalent
///   returns a clean zero count, not an empty slice that triggers
///   the "from_raw_parts with non-null but zero len" debug assert
///   downstream).
///
/// # Section name argument
///
/// `name` must be ≤ 8 bytes — PE section names are fixed 8-byte
/// fields. Names < 8 bytes are zero-padded for comparison so callers
/// pass `b".lt_meta"` (8 bytes including the leading `.`) or
/// `b".lt_skix"` etc.; the function returns `None` immediately on a
/// > 8 byte name (configuration bug; surfaces as a missing section
/// at runtime).
///
/// # Safety
///
/// Safe to call. All pointer dereferences go through
/// `core::ptr::read_unaligned`; range checks bound every read to
/// within the image's mapped pages. The worst-case failure mode on a
/// pathologically corrupt PE is a segfault inside the loader-mapped
/// image — but a binary corrupt enough to break the section walk is
/// also corrupt enough to break `LoadLibrary`, so we'd never reach
/// `luna_aot_run` in that scenario.
pub fn find_section(name: &[u8]) -> Option<(*const u8, usize)> {
    // PE section names cap at 8 bytes. A caller passing a longer name
    // is a bug — the section can't possibly exist with that name in
    // any linked PE. Surface as None rather than truncate-and-match
    // (which would mis-fire on `luna_trace_meta` truncating to
    // `luna_tra` and collide with a hypothetical `luna_trace_blob`
    // truncating to the same prefix).
    if name.len() > 8 {
        return None;
    }

    // Zero-pad the search name to 8 bytes for direct comparison
    // against the on-disk `IMAGE_SECTION_HEADER::Name` field.
    let mut needle = [0u8; 8];
    needle[..name.len()].copy_from_slice(name);

    // SAFETY: `GetModuleHandleW(NULL)` is a documented stable API
    // returning the calling process's image base. The returned
    // HMODULE is the address of the loaded PE image (= DOS header).
    // Returns NULL only under extreme system distress; on success
    // points to readable mapped memory of at least one page.
    let image_base = unsafe { GetModuleHandleW(core::ptr::null()) };
    if image_base.is_null() {
        return None;
    }

    // SAFETY: image_base is non-null and points at the start of a
    // mapped PE image; the DOS header is always the first 64 bytes.
    // `read_unaligned` because PE headers don't honour Rust's
    // alignment requirements (`u16` fields at byte offsets 0/2/4/…).
    let dos: ImageDosHeader = unsafe { core::ptr::read_unaligned(image_base as *const _) };
    // `{ dos.e_magic }` (the brace form) forces a copy out of the
    // packed struct before read — Rust 1.78+ rejects implicit packed-
    // field borrows.
    if { dos.e_magic } != 0x5A4D {
        // "MZ" little-endian. Not a PE — bail.
        return None;
    }
    let e_lfanew = { dos.e_lfanew };
    // Defensive: e_lfanew should land within the first 4 KiB of the
    // image (typical layout puts NT headers at 0x80 - 0x200). A wildly
    // off-range value indicates a stripped or corrupt binary — bail
    // before chasing the pointer.
    if !(0..0x1000).contains(&e_lfanew) {
        return None;
    }

    // NT headers layout:
    //   [u32 signature "PE\0\0"]
    //   [IMAGE_FILE_HEADER]  ← 20 bytes
    //   [IMAGE_OPTIONAL_HEADER]  ← size_of_optional_header bytes
    //   [IMAGE_SECTION_HEADER × number_of_sections]
    //
    // We skip the optional header entirely — its size is carried in
    // the file header.
    // SAFETY: e_lfanew bounded to < 0x1000 above; image_base + 0x1000
    // is within the loader-mapped region for any PE we've ever seen.
    let nt_base = unsafe { image_base.offset(e_lfanew as isize) };
    // SAFETY: read the 4-byte signature; bail if it's not "PE\0\0".
    let pe_sig: u32 = unsafe { core::ptr::read_unaligned(nt_base as *const u32) };
    if pe_sig != 0x0000_4550 {
        // 'P' | ('E' << 8). Not a PE NT-header signature — bail.
        return None;
    }

    // SAFETY: file header sits at nt_base + 4. Total bytes read so far
    // (e_lfanew + 4 + sizeof(ImageFileHeader)) = max ~4116, well within
    // the loader-mapped image.
    let file_hdr_ptr = unsafe { nt_base.add(4) } as *const ImageFileHeader;
    let file_hdr: ImageFileHeader = unsafe { core::ptr::read_unaligned(file_hdr_ptr) };
    let n_sections = { file_hdr.number_of_sections } as usize;
    let opt_hdr_size = { file_hdr.size_of_optional_header } as usize;

    // Defensive: PE optional header is conventionally 224 (PE32) or
    // 240 (PE32+); 0 is legal in obj files but never in linked
    // images. A value > 4096 indicates corruption.
    if opt_hdr_size > 4096 {
        return None;
    }
    if n_sections == 0 || n_sections > 96 {
        // Hard cap: PE format documents up to 96 sections per image.
        // Any larger value is corruption.
        return None;
    }

    // Section table starts immediately after the optional header.
    // SAFETY: cumulative offset = e_lfanew + 4 + 20 + opt_hdr_size +
    // n_sections * 40 = at most 0x1000 + 4 + 20 + 4096 + 96*40 ≈ 9 KiB;
    // within the loader-mapped headers region.
    let section_table_base = unsafe {
        (file_hdr_ptr as *const u8).add(core::mem::size_of::<ImageFileHeader>() + opt_hdr_size)
    } as *const ImageSectionHeader;

    for i in 0..n_sections {
        // SAFETY: i < n_sections ≤ 96; each section header is 40
        // bytes; total = within the headers region bounded above.
        let sec: ImageSectionHeader =
            unsafe { core::ptr::read_unaligned(section_table_base.add(i)) };
        // Compare names byte-for-byte (8-byte field, NUL-padded for
        // short names; full 8 bytes for max-length names).
        if { sec.name } != needle {
            continue;
        }
        let vsize = { sec.virtual_size } as usize;
        let vaddr = { sec.virtual_address } as usize;
        if vsize == 0 {
            // Empty section — same treatment as missing. Caller's
            // bracketed-walk equivalent expects a non-zero base for
            // any present section.
            return None;
        }
        // SAFETY: vaddr is an RVA bounded by the linker's image size
        // computation; image_base + vaddr is the run-time base of
        // section data. The loader applied base relocations during
        // `LoadLibrary`, so any pointers stored *inside* the section
        // are already relocated to absolute addresses.
        let section_base = unsafe { image_base.add(vaddr) } as *const u8;
        return Some((section_base, vsize));
    }
    None
}
