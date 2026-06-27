//! v2.1 R3.3+ Design 2 Phase 0 cargo-asm pre-approval gate harness.
//!
//! Standalone probe: dumps codegen for two candidate "deopt restore" loop
//! shapes side-by-side, so we can read off whether the Design 2 SnapEntry
//! walk gets caught by the same Family-1 NEGATIVE pattern that killed
//! A8 / O04 / hash-polish / S07 (see `.dev/rfcs/v2.0-pi-phase2-closure.md`
//! §"Family 1 — LLVM CSE pessimize").
//!
//! Family-1 pattern recap: 4 attacks added a cmp/branch/load to the
//! Vm::run hot path (frame-fetch hoist, cache classifier, tag-byte
//! pre-check, inline tail). All 4 went NEGATIVE because rustc/LLVM had
//! already folded the hot path optimally, and the "helpful" insertion
//! added a load LLVM couldn't CSE back into the discriminant byte.
//!
//! The deopt arm restore loop today (exec.rs:6937-7104) walks
//! `exit_tags_for_pc: &[ExitTag]` (one entry per slot, enum repr u8)
//! and for each slot: match → tag byte → pack(tag, reg_state[i]).
//!
//! Design 2 (v2.0-track-r-r3-3-design-space.md §2) replaces this with
//! a snapshot walk: `SnapEntry { slot:u8, flags:u8, ir_ref:u16 }` array
//! per guard. Per-slot becomes per-touched-slot (sparse), and the
//! payload load swaps from `reg_state[i]` (direct slot index) to
//! `reg_state[ir_ref]` (indirect var index — Cranelift `Variable`).
//!
//! This harness emits both shapes under `#[inline(never)]` + `no_mangle`
//! so `cargo asm` can read them cleanly. The verdict doc compares the
//! arm64 codegen against the Family-1 NEGATIVE pattern.

#![allow(dead_code)]

use std::hint::black_box;

// ---- Mock value layout (mirrors luna's runtime::Value packing) ----

#[repr(C)]
#[derive(Clone, Copy)]
pub union RawVal {
    pub zero: u64,
    pub int: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PackedValue {
    pub tag: u8,
    pub _pad: [u8; 7],
    pub raw: RawVal,
}

// Tag byte constants — copy of luna `runtime::value::raw::*`.
pub const TAG_NIL: u8 = 0;
pub const TAG_INT: u8 = 1;
pub const TAG_FLOAT: u8 = 2;
pub const TAG_TABLE: u8 = 3;
pub const TAG_CLOSURE: u8 = 4;
pub const TAG_NATIVE: u8 = 5;
pub const TAG_STR: u8 = 6;

#[inline]
fn pack(tag: u8, raw_zero: u64) -> PackedValue {
    PackedValue {
        tag,
        _pad: [0; 7],
        raw: RawVal { zero: raw_zero },
    }
}

// ===================================================================
// Shape A — current deopt restore loop (mirrors exec.rs:7049-7104)
// ===================================================================

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum ExitTag {
    Untouched = 0,
    Int = 1,
    Float = 2,
    Table = 3,
    Closure = 4,
    Nil = 5,
    Str = 6,
}

#[inline(never)]
#[unsafe(no_mangle)]
pub fn walk_current_style(
    exit_tags_for_pc: &[ExitTag],
    entry_tags: &[u8],
    reg_state: &[i64],
    stack: &mut [PackedValue],
    base_us: usize,
    max_stack: usize,
) {
    let slot_count = exit_tags_for_pc.len();
    for i in 0..slot_count {
        let tag = match exit_tags_for_pc[i] {
            ExitTag::Untouched => {
                if i < max_stack {
                    entry_tags[i]
                } else {
                    TAG_NIL
                }
            }
            ExitTag::Int => TAG_INT,
            ExitTag::Float => TAG_FLOAT,
            ExitTag::Table => TAG_TABLE,
            ExitTag::Closure => TAG_CLOSURE,
            ExitTag::Nil => TAG_NIL,
            ExitTag::Str => TAG_STR,
        };
        stack[base_us + i] = pack(tag, reg_state[i] as u64);
    }
}

// ===================================================================
// Shape B — Design 2 snapshot walk
// ===================================================================
//
// Per design-space.md §2:
//   pub struct SnapEntry { slot: u8, flags: u8, ir_ref: u16 }
//   pub struct Snapshot { entries: Box<[SnapEntry]>, ... }
//
// Walk:
//   for entry in snap.entries {
//     let tag = derive_tag_from_flags(entry.flags);
//     let v = reg_state[entry.ir_ref as usize];  // indirect var idx
//     stack[entry.slot as usize] = pack(tag, v);
//   }
//
// Note "sparse": only slots that the trace TOUCHED appear in the snap,
// the rest implicitly retain entry-state. Compare to Shape A's dense
// per-slot iter over the whole window.

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SnapEntry {
    pub slot: u8,
    pub flags: u8,
    pub ir_ref: u16,
}

// `flags` low 3 bits = tag-class. Mirrors ExitTag's payload encoding
// but stored inside SnapEntry rather than a separate [ExitTag] slice.
const FLAG_TAG_MASK: u8 = 0x07;
const FLAG_TAG_INT: u8 = 1;
const FLAG_TAG_FLOAT: u8 = 2;
const FLAG_TAG_TABLE: u8 = 3;
const FLAG_TAG_CLOSURE: u8 = 4;
const FLAG_TAG_NIL: u8 = 5;
const FLAG_TAG_STR: u8 = 6;
// flag bit 3 = "untouched, fall back to entry_tags[slot]"
const FLAG_UNTOUCHED: u8 = 0x08;

#[inline(never)]
#[unsafe(no_mangle)]
pub fn walk_snapshot_style(
    snap_entries: &[SnapEntry],
    entry_tags: &[u8],
    reg_state: &[i64],
    stack: &mut [PackedValue],
    base_us: usize,
) {
    for e in snap_entries {
        let raw_tag = e.flags & FLAG_TAG_MASK;
        let tag = if e.flags & FLAG_UNTOUCHED != 0 {
            entry_tags[e.slot as usize]
        } else {
            match raw_tag {
                FLAG_TAG_INT => TAG_INT,
                FLAG_TAG_FLOAT => TAG_FLOAT,
                FLAG_TAG_TABLE => TAG_TABLE,
                FLAG_TAG_CLOSURE => TAG_CLOSURE,
                FLAG_TAG_NIL => TAG_NIL,
                FLAG_TAG_STR => TAG_STR,
                _ => TAG_NIL,
            }
        };
        let v = reg_state[e.ir_ref as usize] as u64;
        stack[base_us + e.slot as usize] = pack(tag, v);
    }
}

// ===================================================================
// Shape B' — Design 2 snapshot walk with a "no ir_ref indirection"
// variant: ir_ref == slot. Used to isolate the cost of the indirect
// load alone (var_idx → reg_state) from the rest of the SnapEntry
// shape change.
// ===================================================================

#[inline(never)]
#[unsafe(no_mangle)]
pub fn walk_snapshot_style_direct(
    snap_entries: &[SnapEntry],
    entry_tags: &[u8],
    reg_state: &[i64],
    stack: &mut [PackedValue],
    base_us: usize,
) {
    for e in snap_entries {
        let raw_tag = e.flags & FLAG_TAG_MASK;
        let tag = if e.flags & FLAG_UNTOUCHED != 0 {
            entry_tags[e.slot as usize]
        } else {
            match raw_tag {
                FLAG_TAG_INT => TAG_INT,
                FLAG_TAG_FLOAT => TAG_FLOAT,
                FLAG_TAG_TABLE => TAG_TABLE,
                FLAG_TAG_CLOSURE => TAG_CLOSURE,
                FLAG_TAG_NIL => TAG_NIL,
                FLAG_TAG_STR => TAG_STR,
                _ => TAG_NIL,
            }
        };
        let idx = e.slot as usize;
        let v = reg_state[idx] as u64;
        stack[base_us + idx] = pack(tag, v);
    }
}

// ===================================================================
// Driver — keep all three fns alive for asm dump.
// ===================================================================

fn main() {
    // Shape A — dense ExitTag slice (window_size = 8 typical for token_bucket)
    let exit_tags_a: Vec<ExitTag> = vec![
        ExitTag::Int,
        ExitTag::Untouched,
        ExitTag::Int,
        ExitTag::Float,
        ExitTag::Table,
        ExitTag::Untouched,
        ExitTag::Closure,
        ExitTag::Nil,
    ];
    let entry_tags: Vec<u8> = vec![
        TAG_INT, TAG_FLOAT, TAG_TABLE, TAG_NIL, TAG_INT, TAG_STR, TAG_CLOSURE, TAG_INT,
    ];
    let reg_state: Vec<i64> = vec![100, 200, 300, 400, 500, 600, 700, 800];
    let mut stack_a: Vec<PackedValue> = vec![pack(TAG_NIL, 0); 16];

    walk_current_style(
        black_box(&exit_tags_a),
        black_box(&entry_tags),
        black_box(&reg_state),
        black_box(&mut stack_a),
        black_box(0),
        black_box(8),
    );

    // Shape B/B' — sparse SnapEntries (only 4 touched slots out of 8)
    let snap_entries: Vec<SnapEntry> = vec![
        SnapEntry { slot: 0, flags: FLAG_TAG_INT, ir_ref: 0 },
        SnapEntry { slot: 2, flags: FLAG_TAG_INT, ir_ref: 2 },
        SnapEntry { slot: 3, flags: FLAG_TAG_FLOAT, ir_ref: 3 },
        SnapEntry { slot: 6, flags: FLAG_TAG_CLOSURE, ir_ref: 6 },
    ];
    let mut stack_b: Vec<PackedValue> = vec![pack(TAG_NIL, 0); 16];
    walk_snapshot_style(
        black_box(&snap_entries),
        black_box(&entry_tags),
        black_box(&reg_state),
        black_box(&mut stack_b),
        black_box(0),
    );

    let mut stack_b2: Vec<PackedValue> = vec![pack(TAG_NIL, 0); 16];
    walk_snapshot_style_direct(
        black_box(&snap_entries),
        black_box(&entry_tags),
        black_box(&reg_state),
        black_box(&mut stack_b2),
        black_box(0),
    );

    // Force outputs alive so DCE doesn't strip the calls.
    black_box(&stack_a);
    black_box(&stack_b);
    black_box(&stack_b2);
    println!("probe_design2_snapshot_walk: ok");
}
