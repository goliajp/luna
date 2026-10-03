//! Deploy-side proto slot resolver.
//!
//! An AOT trace that inlined a call to another function checks the
//! callee's prototype against a pointer it loads from a writable 8-byte
//! slot (`__luna_aot_proto_slot_<hash>`). Each slot has a 16-byte
//! `[hash_addr, slot_addr]` entry in section `luna_proto_idx` (`.lt_prix`
//! on Windows), where `hash_addr` points at the prototype's
//! `Proto::stable_hash`. After `vm.load`, this walks the section and writes
//! into each slot the loaded chunk's prototype of that hash. A slot whose
//! hash matches none stays null, which no closure's prototype equals: the
//! trace then leaves at that call. The section is found as in
//! [`crate::aot_strkey_resolver`].
use luna_core::runtime::Gc;
use luna_core::runtime::function::Proto;
use luna_core::vm::Vm;

/// One index entry, as `emit_proto_arg` lays it out.
#[repr(C)]
struct IndexEntry {
    /// The 16-byte stable hash of the prototype.
    hash_ptr: *const [u8; 16],
    /// The slot the trace loads the prototype's address from.
    slot_ptr: *mut *const u8,
}

// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(all(unix, not(target_vendor = "apple")))]
unsafe extern "C" {
    #[link_name = "__start_luna_proto_idx"]
    static mut LUNA_PROTO_IDX_START: u8;
    #[link_name = "__stop_luna_proto_idx"]
    static mut LUNA_PROTO_IDX_END: u8;
}

// SAFETY: the linker defines both symbols, at the start and the end of the
// section; they are declared as bytes and only their addresses are taken
#[cfg(target_vendor = "apple")]
unsafe extern "C" {
    #[link_name = "\u{1}section$start$__DATA$luna_proto_idx"]
    static mut LUNA_PROTO_IDX_START: u8;
    #[link_name = "\u{1}section$end$__DATA$luna_proto_idx"]
    static mut LUNA_PROTO_IDX_END: u8;
}

/// Fill every proto slot from the prototypes under `root`; returns how
/// many were filled.
pub fn resolve_all(vm: &Vm, root: Gc<Proto>) -> usize {
    let (base, len_bytes): (*const u8, usize) = {
        #[cfg(target_os = "windows")]
        {
            match crate::windows_section::find_section(b".lt_prix") {
                Some((b, l)) => (b, l),
                None => return 0,
            }
        }
        #[cfg(all(not(target_os = "windows"), unix))]
        {
            let start = &raw mut LUNA_PROTO_IDX_START as *mut IndexEntry;
            let end = &raw mut LUNA_PROTO_IDX_END as *mut IndexEntry;
            let len = (end as isize) - (start as isize);
            if len <= 0 {
                return 0;
            }
            (start as *const u8, len as usize)
        }
        #[cfg(not(any(target_os = "windows", unix)))]
        {
            let _ = (vm, root);
            return 0;
        }
    };
    let protos = vm.collect_proto_hashes(root);
    let n = len_bytes / core::mem::size_of::<IndexEntry>();
    // SAFETY: `(base, len_bytes)` bounds the `luna_proto_idx` section
    // (`.lt_prix` on Windows), which `luna-aot` fills only with aligned
    // `IndexEntry`s and the zero placeholder
    let entries = unsafe { core::slice::from_raw_parts(base as *const IndexEntry, n) };
    let mut filled = 0;
    for e in entries {
        if e.hash_ptr.is_null() || e.slot_ptr.is_null() {
            continue;
        }
        // SAFETY: a non-null `hash_ptr` points at the 16 hash bytes
        let hash = unsafe { *e.hash_ptr };
        if let Some((p, _)) = protos.iter().find(|(_, h)| *h == hash) {
            // SAFETY: a non-null `slot_ptr` points at a writable pointer slot
            unsafe { core::ptr::write(e.slot_ptr, p.as_ptr() as *const u8) };
            filled += 1;
        }
    }
    filled
}
