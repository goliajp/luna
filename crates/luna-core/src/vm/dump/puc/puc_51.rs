//! PUC Lua 5.1 `.luac` → luna `Proto` translator (Phase LB Wave 2).
//!
//! 5.1 is **the hardest dialect** per `.dev/rfcs/v1.3-audit-puc-luac-formats.md`
//! despite being the oldest, because of three distinguishing features
//! luna's opcode set doesn't share with the source format:
//!
//! 1. **6-bit opcode field** — 5.1 instructions are `op:6 | A:8 | C:9 | B:9`
//!    (LE within the u32); luna is `op:7 | A:8 | k:1 | B:8 | C:8`. A
//!    per-instruction decode shim translates the raw word into a
//!    `Pre51Inst` struct before the opcode table picks how to re-encode
//!    each one in luna's layout.
//!
//! 2. **`OP_GETGLOBAL` / `OP_SETGLOBAL` → `_ENV` upvalue synthesis** —
//!    5.1 globals are a special VM-level "globals table" indirection; 5.2
//!    introduced `_ENV` as a proper upvalue and 5.3+/luna inherited that.
//!    The translator synthesises an `_ENV` upvalue (at index 0) in every
//!    Proto that touches globals, then rewrites `GETGLOBAL Bx` →
//!    `GetTabUp(A, _ENV_idx, Bx)` and `SETGLOBAL Bx` →
//!    `SetTabUp(_ENV_idx, Bx, A)`. The top-level chunk's `_ENV` comes
//!    from `Vm::load` (it wraps the main proto in a closure whose
//!    `_ENV` upvalue is the globals table); nested protos capture their
//!    parent's `_ENV` (`in_stack=false, index=parent_env_idx`).
//!
//! 3. **`OP_CLOSURE` pseudo-instruction strip + PC patch** — in 5.1, each
//!    `OP_CLOSURE A Bx` is **followed** by `nups` pseudo-instructions
//!    (`OP_MOVE 0 B 0` or `OP_GETUPVAL 0 B 0`) that aren't actually
//!    executed; PUC's VM treats them as opaque upvalue descriptors and
//!    bumps `pc` past them. luna's `Op::Closure` reads upvalue capture
//!    from `Proto.upvals` (the 5.2+ inline-desc convention), so the
//!    pseudo-instructions must be: (a) read into `UpvalDesc` rows on the
//!    *nested* proto, (b) stripped from the parent's code stream, and
//!    (c) every subsequent `Jmp` / `FORLOOP` / `FORPREP` target adjusted
//!    by the cumulative strip count so jumps land where they used to.
//!
//! ## Punts (documented for follow-up, not silently dropped)
//!
//! - **`OP_TFORLOOP` 3-way split** — 5.1's single `TFORLOOP` op
//!   (A C, no separate TFORPREP / TFORCALL) needs splitting into luna's
//!   `TForPrep + TForCall + TForLoop` triad. Translator currently
//!   rejects with `unsupported PUC 5.1 op TFORLOOP`. Affects generic
//!   `for k,v in pairs(t) do … end` loops. Tracked: punt-A.
//! - **`LUAI_COMPAT_VARARG` `arg` local** — 5.1 vararg functions
//!   compiled with the compat flag set the `is_vararg` byte's bit 2
//!   (`NEEDSARG`). Translator decodes the bit into
//!   `Proto.has_compat_vararg_arg` but does NOT yet populate the
//!   synthetic `arg` table at runtime — chunks that reference `arg`
//!   from a `...` function will see nil. Tracked: punt-B.
//! - **`luaO_fb2int` for `NEWTABLE` size hints** — 5.1 packs the array
//!   and hash size hints as floating-byte (8-bit mantissa+exponent);
//!   translator decodes them naïvely (saturates to 0 if the decoded
//!   value exceeds luna's 8-bit B/C field). Correctness-safe (hints are
//!   advisory) but may pessimise table allocation. Tracked: punt-C.
//! - **`LOADBOOL A B C` → `LFalseSkip` split** — 5.1's `LOADBOOL` has a
//!   `pc++` form (when C != 0). Translator handles the common
//!   `LOADBOOL A 0 0` / `LOADBOOL A 1 0` cases via `LoadFalse` /
//!   `LoadTrue`; the `C != 0` skip form is rejected. Affects `(a == b)`
//!   in boolean position; not common in straight-line code but the
//!   short-circuit operators do emit it. Tracked: punt-D.
//!
//! See `.dev/rfcs/v1.3-audit-puc-luac-formats.md` §"5.1 risks" for the
//! full deferred-work list.

use super::super::reader::Reader;
use crate::runtime::Value;
use crate::runtime::function::{LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::vm::isa::{Inst, Op};

/// PUC 5.1 binary-chunk header (12 bytes). Differs from 5.2+ in carrying
/// an explicit endian flag (byte 6, `1` = LE) and an integral-vs-float
/// flag (byte 11, `0` = floating-point `lua_Number`). luna requires LE +
/// f64; anything else is rejected with a clear error per RFC v1.3
/// §"Cross-dialect risks" item 1 ("endianness — enforce LE, reject BE").
const HEADER_LEN: usize = 12;

/// Decoded PUC 5.1 instruction fields. The raw u32 word's bit layout is
/// `op:6 | A:8 | C:9 | B:9` (LE within the u32), so the decoder
/// unpacks once and the translator never re-touches the raw bits.
#[derive(Clone, Copy, Debug)]
struct Pre51Inst {
    op: u8,
    a: u32,
    /// B field (9 bits, top bit = "RK" flag — bit set ⇒ K-pool index in low 8 bits).
    b: u32,
    /// C field (9 bits, top bit = "RK" flag — bit set ⇒ K-pool index in low 8 bits).
    c: u32,
    /// Unsigned Bx (18 bits = C<<9 | B).
    bx: u32,
    /// Signed sBx (Bx - 131071).
    sbx: i32,
}

const PRE51_BITRK: u32 = 1 << 8;
const PRE51_MAXARG_BX: u32 = (1 << 18) - 1;
const PRE51_MAXARG_SBX: i32 = (PRE51_MAXARG_BX >> 1) as i32; // 131071

fn decode_inst_51(raw: u32) -> Pre51Inst {
    let op = (raw & 0x3F) as u8;
    let a = (raw >> 6) & 0xFF;
    let c = (raw >> 14) & 0x1FF;
    let b = (raw >> 23) & 0x1FF;
    let bx = (raw >> 14) & PRE51_MAXARG_BX;
    let sbx = bx as i32 - PRE51_MAXARG_SBX;
    Pre51Inst {
        op,
        a,
        b,
        c,
        bx,
        sbx,
    }
}

// PUC 5.1 opcode numbers (lopcodes.h 5.1.5).
const OP_MOVE: u8 = 0;
const OP_LOADK: u8 = 1;
const OP_LOADBOOL: u8 = 2;
const OP_LOADNIL: u8 = 3;
const OP_GETUPVAL: u8 = 4;
const OP_GETGLOBAL: u8 = 5;
const OP_GETTABLE: u8 = 6;
const OP_SETGLOBAL: u8 = 7;
const OP_SETUPVAL: u8 = 8;
const OP_SETTABLE: u8 = 9;
const OP_NEWTABLE: u8 = 10;
const OP_SELF: u8 = 11;
const OP_ADD: u8 = 12;
const OP_SUB: u8 = 13;
const OP_MUL: u8 = 14;
const OP_DIV: u8 = 15;
const OP_MOD: u8 = 16;
const OP_POW: u8 = 17;
const OP_UNM: u8 = 18;
const OP_NOT: u8 = 19;
const OP_LEN: u8 = 20;
const OP_CONCAT: u8 = 21;
const OP_JMP: u8 = 22;
const OP_EQ: u8 = 23;
const OP_LT: u8 = 24;
const OP_LE: u8 = 25;
const OP_TEST: u8 = 26;
const OP_TESTSET: u8 = 27;
const OP_CALL: u8 = 28;
const OP_TAILCALL: u8 = 29;
const OP_RETURN: u8 = 30;
const OP_FORLOOP: u8 = 31;
const OP_FORPREP: u8 = 32;
const OP_TFORLOOP: u8 = 33;
const OP_SETLIST: u8 = 34;
const OP_CLOSE: u8 = 35;
const OP_CLOSURE: u8 = 36;
const OP_VARARG: u8 = 37;

/// Entry point. Decodes the 5.1 header, then recurses through the proto
/// tree producing a `Gc<Proto>` with luna-native opcodes.
pub(in crate::vm::dump) fn undump(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    validate_header(bytes)?;
    let mut r = Reader::at(bytes, HEADER_LEN);
    let proto = r_proto(&mut r, heap, None)?;
    // PUC dump trailing-byte check: 5.1 doesn't always pad, but any leftover
    // is a sign the decoder mis-sized something earlier.
    if r.pos() != bytes.len() {
        return Err(format!(
            "trailing bytes in PUC 5.1 chunk (consumed {}, total {})",
            r.pos(),
            bytes.len()
        ));
    }
    Ok(proto)
}

fn validate_header(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < HEADER_LEN {
        return Err("truncated PUC 5.1 binary chunk header".to_string());
    }
    if &bytes[0..4] != b"\x1bLua" {
        return Err("bad PUC 5.1 signature".to_string());
    }
    if bytes[4] != 0x51 {
        return Err(format!(
            "expected PUC 5.1 version byte 0x51, got 0x{:02x}",
            bytes[4]
        ));
    }
    if bytes[5] != 0x00 {
        return Err(format!(
            "unsupported PUC 5.1 format byte 0x{:02x}",
            bytes[5]
        ));
    }
    if bytes[6] != 0x01 {
        return Err("luna only supports little-endian PUC 5.1 chunks".to_string());
    }
    if bytes[7] != 4 {
        return Err(format!("PUC 5.1 sizeof(int) must be 4, got {}", bytes[7]));
    }
    if bytes[8] != 8 {
        return Err(format!(
            "PUC 5.1 sizeof(size_t) must be 8, got {}",
            bytes[8]
        ));
    }
    if bytes[9] != 4 {
        return Err(format!(
            "PUC 5.1 sizeof(Instruction) must be 4, got {}",
            bytes[9]
        ));
    }
    if bytes[10] != 8 {
        return Err(format!(
            "PUC 5.1 sizeof(lua_Number) must be 8, got {}",
            bytes[10]
        ));
    }
    if bytes[11] != 0 {
        return Err(
            "luna only supports floating-point PUC 5.1 chunks (integral build rejected)"
                .to_string(),
        );
    }
    Ok(())
}

// PUC 5.1 stores strings as size_t (8 bytes LE) followed by the bytes
// *including* a trailing `\0`. Empty strings serialize as size==0 with no
// payload (PUC: "if size == 0 then nullptr"). We strip the trailing NUL
// when present so luna's interned strings don't carry it.
fn r_string_51<'a>(r: &mut Reader<'a>) -> Result<&'a [u8], String> {
    let n = u64::from_le_bytes(r.take(8)?.try_into().unwrap()) as usize;
    if n == 0 {
        return Ok(&[]);
    }
    let bytes = r.take(n)?;
    // Drop the PUC trailing `\0` (always present when n > 0).
    if let Some((b'\0', rest)) = bytes.split_last() {
        Ok(rest)
    } else {
        Ok(bytes)
    }
}

fn r_int_51(r: &mut Reader) -> Result<i32, String> {
    Ok(i32::from_le_bytes(r.take(4)?.try_into().unwrap()))
}

fn r_number_51(r: &mut Reader) -> Result<f64, String> {
    Ok(f64::from_bits(u64::from_le_bytes(
        r.take(8)?.try_into().unwrap(),
    )))
}

fn r_const_51(r: &mut Reader, heap: &mut Heap) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(r.u8()? != 0),
        // 5.1 has no integer subtype; LUA_TNUMBER is always f64.
        3 => Value::Float(r_number_51(r)?),
        4 => {
            let s = r_string_51(r)?;
            Value::Str(heap.intern(s))
        }
        t => return Err(format!("bad PUC 5.1 constant tag {t}")),
    })
}

/// Decode + translate one proto (recursive for nested protos referenced
/// by `OP_CLOSURE`). `parent_env_idx` is the index of `_ENV` in the
/// parent proto's upvalue list, used when a nested proto's `_ENV` is
/// synthesised from the parent's `_ENV` rather than the host globals.
fn r_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_env_idx: Option<u8>,
) -> Result<Gc<Proto>, String> {
    // PUC 5.1 LoadFunction order:
    //   string source / int line_defined / int last_line_defined /
    //   byte nups / byte numparams / byte is_vararg / byte max_stack /
    //   code / constants / protos / lineinfo / locvars / upvalnames
    let source_raw = r_string_51(r)?;
    let source = heap.intern(source_raw);
    let line_defined = r_int_51(r)?.max(0) as u32;
    let last_line_defined = r_int_51(r)?.max(0) as u32;
    let nups = r.u8()? as usize;
    let num_params = r.u8()?;
    // 5.1 is_vararg byte: bit 0 HASARG, bit 1 ISVARARG, bit 2 NEEDSARG.
    let vararg_byte = r.u8()?;
    let is_vararg = (vararg_byte & 0x02) != 0;
    let has_compat_vararg_arg = (vararg_byte & 0x04) != 0; // punt-B: decoded but not honoured at runtime yet
    let max_stack = r.u8()?;

    // --- code ---
    let n_code = r_int_51(r)?.max(0) as usize;
    let mut raw_code = Vec::with_capacity(n_code);
    for _ in 0..n_code {
        raw_code.push(decode_inst_51(r.u32()?));
    }

    // --- constants ---
    let n_consts = r_int_51(r)?.max(0) as usize;
    let mut consts: Vec<Value> = Vec::with_capacity(n_consts);
    for _ in 0..n_consts {
        consts.push(r_const_51(r, heap)?);
    }

    // --- nested protos (recurse) ---
    // Note: we recurse BEFORE translating the parent code because the
    // CLOSURE pseudo-instruction strip needs to consume nups-of-child
    // pseudo-instructions per nested proto; we'll compute the upvalue
    // descriptors from the parent's pseudo-instructions then attach
    // them to the nested protos in a second pass.
    let n_protos = r_int_51(r)?.max(0) as usize;
    // We need to know each child's nups (= len(child.upvals)) *before* the
    // strip pass — but at PUC 5.1 dump time the child's upvalue list isn't
    // stored as a `nups` count up front; it's implicit from the parent's
    // pseudo-instructions. Each nested proto carries its *own* nups byte
    // (already decoded above as the recursive call's `nups`), which equals
    // the number of pseudo-instructions following its OP_CLOSURE in the
    // parent's code. So: recurse first, capture each child's `nups`, then
    // strip that many pseudo-instructions out of `raw_code` after each
    // OP_CLOSURE.
    //
    // We still need parent_env_idx for nested protos that synthesise their
    // own `_ENV` from ours — but at this point we don't *yet* know whether
    // *this* proto will synthesise `_ENV` (depends on whether it touches
    // globals). Two-pass: do a global-scan first.
    let needs_env = raw_code
        .iter()
        .any(|i| matches!(i.op, OP_GETGLOBAL | OP_SETGLOBAL));
    // Even when this proto doesn't touch globals itself, we still synth
    // `_ENV` if ANY descendant needs it — but determining that requires
    // recursing first. Simpler: synth `_ENV` unconditionally for the main
    // chunk (parent_env_idx.is_none()) and on-demand for nested protos.
    // For now: synth `_ENV` iff this proto needs it OR it's the main chunk.
    // (Nested protos that don't touch globals get the cheaper layout.)
    let synth_env = needs_env || parent_env_idx.is_none();

    // Synthesised `_ENV` lands at index 0 of upvals; the original PUC 5.1
    // upvalues (named via the upvalnames section at the tail of the proto)
    // get shifted +1. We pre-allocate the slot and patch the `index` on
    // GetUpval / SetUpval translations accordingly.
    let env_shift: u8 = if synth_env { 1 } else { 0 };
    let mut upvals: Vec<UpvalDesc> = Vec::with_capacity(nups + env_shift as usize);
    if synth_env {
        // For the main chunk, `Vm::load`'s wrap-in-closure path fills upval
        // 0 with the host globals table (matching how it does this for
        // luna's own dumps when env_upval_idx==0). For nested protos, we
        // capture from the parent's `_ENV` slot.
        let (in_stack, index) = match parent_env_idx {
            None => (false, 0),            // main chunk — Vm::load supplies globals
            Some(p_env) => (false, p_env), // nested — chain from parent's _ENV
        };
        upvals.push(UpvalDesc {
            in_stack,
            index,
            name: "_ENV".to_string().into_boxed_str(),
            read_only: false,
        });
    }
    // The original PUC upvalue rows arrive later in the chunk (after
    // protos + lineinfo + locvars). We reserve their slots now and fill in
    // names / descriptors as we encounter the per-CLOSURE pseudo-
    // instructions in the parent's code below.
    for _ in 0..nups {
        upvals.push(UpvalDesc {
            in_stack: false,
            index: 0,
            name: "".to_string().into_boxed_str(),
            read_only: false,
        });
    }

    // Recurse into nested protos. The parent_env_idx we pass equals our
    // own `_ENV` slot (0) iff we synthesised one; else None (meaning
    // nested children that touch globals would need to synth one of their
    // own — but they'd have no way to reach our globals; this is the rare
    // "function never sees globals AND no descendant does" case, fine).
    let our_env_idx_for_children: Option<u8> = if synth_env { Some(0) } else { parent_env_idx };
    let mut protos: Vec<Gc<Proto>> = Vec::with_capacity(n_protos);
    // child_nups[i] = number of pseudo-instructions following OP_CLOSURE
    // for the i-th nested proto. Used by the strip pass below.
    let mut child_nups: Vec<usize> = Vec::with_capacity(n_protos);
    for _ in 0..n_protos {
        // Each nested proto's nups byte is the 4th byte of its header, so
        // r_proto recursion will consume it. We capture it via the
        // returned Proto's upvals length (after `_ENV` synth that's
        // `len(upvals) - env_shift_of_child`, but the child's strip-time
        // pseudo-instruction count == its *PUC* nups, not its luna upvals
        // count). To recover the original PUC nups: store it in a Cell.
        // Simplest workaround: re-peek the nups byte before recursing.
        // (PUC 5.1 nups byte is at offset: after source string + 8 bytes
        // of line_defined/last_line_defined.) We don't want to manually
        // re-parse — instead, defer: after recursion, ask the child for
        // the *original* nups via a translator-internal sidechannel.
        //
        // Pragmatic solution: temporarily store the PUC nups in the
        // child's `max_stack` upper-bits? No — that breaks `max_stack`.
        // Instead, recurse and have r_proto return `(Gc<Proto>, puc_nups)`.
        let (child, puc_nups) = r_proto_with_puc_nups(r, heap, our_env_idx_for_children)?;
        protos.push(child);
        child_nups.push(puc_nups);
    }

    // --- lineinfo ---
    let n_lines = r_int_51(r)?.max(0) as usize;
    let mut raw_lines: Vec<u32> = Vec::with_capacity(n_lines);
    for _ in 0..n_lines {
        raw_lines.push(r_int_51(r)?.max(0) as u32);
    }

    // --- locvars ---
    let n_loc = r_int_51(r)?.max(0) as usize;
    let mut locvars: Vec<LocVar> = Vec::with_capacity(n_loc);
    for _ in 0..n_loc {
        let name = String::from_utf8_lossy(r_string_51(r)?)
            .into_owned()
            .into_boxed_str();
        let start_pc = r_int_51(r)?.max(0) as u32;
        let end_pc = r_int_51(r)?.max(0) as u32;
        locvars.push(LocVar {
            name,
            // PUC 5.1 LocVar doesn't carry a register — luna defaults to 0
            // here (debug-only field; locvar registers can be reconstructed
            // post-hoc from the source position but it's outside this
            // translator's scope).
            reg: 0,
            start_pc,
            end_pc,
        });
    }

    // --- upvalue names (5.1 stores names separately, in declaration order
    //     matching the indices used by GETUPVAL/SETUPVAL).
    let n_upnames = r_int_51(r)?.max(0) as usize;
    if n_upnames != nups && n_upnames != 0 {
        return Err(format!(
            "PUC 5.1 upvalue-name count {n_upnames} ≠ nups {nups}"
        ));
    }
    for i in 0..n_upnames {
        let name = String::from_utf8_lossy(r_string_51(r)?)
            .into_owned()
            .into_boxed_str();
        // upvals[env_shift + i] is the original PUC upvalue slot i.
        upvals[env_shift as usize + i].name = name;
    }

    // --- translate code + strip CLOSURE pseudo-instructions ---
    let (code, lines) = translate_code(
        &raw_code,
        &raw_lines,
        &child_nups,
        env_shift,
        &mut upvals,
        &protos,
        consts.len(),
    )?;

    let env_upval_idx = upvals
        .iter()
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);

    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: code.into_boxed_slice(),
        consts: consts.into_boxed_slice(),
        protos: protos.into_boxed_slice(),
        upvals: upvals.into_boxed_slice(),
        num_params,
        is_vararg,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg,
        max_stack,
        lines: lines.into_boxed_slice(),
        source,
        line_defined,
        last_line_defined,
        locvars: locvars.into_boxed_slice(),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        traces: std::cell::RefCell::new(Vec::new()),
    }))
}

/// Recurse into a nested proto AND return its PUC-5.1 nups (the strip
/// pass needs this to know how many pseudo-instructions follow the
/// parent's `OP_CLOSURE` for this child). r_proto can't return it on its
/// own because the field gets folded into `upvals.len()` post-translation.
fn r_proto_with_puc_nups(
    r: &mut Reader,
    heap: &mut Heap,
    parent_env_idx: Option<u8>,
) -> Result<(Gc<Proto>, usize), String> {
    // Snapshot the reader position so we can peek the nups byte; then
    // rewind by calling r_proto from the original position. Reader has
    // no rewind API, so instead: parse the leading source string, the two
    // line ints, then the nups byte, BUT we still need to fully decode
    // the proto. Simpler: peek without consuming.
    //
    // Strategy: parse the source-string length to skip it, then the two
    // ints, peek the nups byte, then rewind by re-creating a Reader at
    // the saved position. Reader exposes `.pos()` + `Reader::at`, so the
    // rewind works.
    let saved = r.pos();
    // peek source length
    let _src = r_string_51(r)?;
    let _ld = r_int_51(r)?;
    let _lld = r_int_51(r)?;
    let nups = r.u8()? as usize;
    // We've inspected past `nups`. Rewind to `saved` and let r_proto
    // re-parse properly.
    let rewind_bytes = r.peek_underlying_slice();
    let mut rewound = Reader::at(rewind_bytes, saved);
    let proto = r_proto(&mut rewound, heap, parent_env_idx)?;
    // Advance the outer reader to the new position the rewound reader
    // landed on.
    let new_pos = rewound.pos();
    // forward `r` by reading (and discarding) the bytes we just covered
    r.skip_to(new_pos)?;
    Ok((proto, nups))
}

/// Translate the PUC 5.1 instruction stream into luna's opcode set,
/// stripping `OP_CLOSURE` pseudo-instructions and patching jump targets
/// accordingly.
fn translate_code(
    raw_code: &[Pre51Inst],
    raw_lines: &[u32],
    child_nups: &[usize],
    env_shift: u8,
    upvals: &mut [UpvalDesc],
    protos: &[Gc<Proto>],
    n_consts: usize,
) -> Result<(Vec<Inst>, Vec<u32>), String> {
    // Pass 1: walk raw_code, building (old_pc → new_pc) map. For each
    // OP_CLOSURE, consume the following nups pseudo-instructions and
    // record them in the child proto's upvalue list. Pseudo-instructions
    // are NOT emitted; new_pc advances by 1 for the CLOSURE itself.
    let mut new_pc_for: Vec<i64> = Vec::with_capacity(raw_code.len());
    // -1 sentinel = "this old pc is a stripped pseudo-instruction; jumps
    // to it are illegal". The PUC compiler never emits a jump targeting a
    // pseudo-instruction, so any non -1 lookup is a soundness check.
    let mut closure_idx = 0usize;
    let mut new_pc = 0i64;
    let mut i = 0usize;
    while i < raw_code.len() {
        let inst = raw_code[i];
        if inst.op == OP_CLOSURE {
            new_pc_for.push(new_pc);
            new_pc += 1;
            // Strip `nups` pseudo-instructions.
            if closure_idx >= child_nups.len() {
                return Err(format!(
                    "OP_CLOSURE #{} has no matching nested proto",
                    closure_idx
                ));
            }
            let n = child_nups[closure_idx];
            closure_idx += 1;
            for j in 1..=n {
                if i + j >= raw_code.len() {
                    return Err("OP_CLOSURE pseudo-instructions truncate the code stream".into());
                }
                new_pc_for.push(-1); // pseudo-instruction → no luna PC
            }
            i += 1 + n;
        } else {
            new_pc_for.push(new_pc);
            new_pc += 1;
            i += 1;
        }
    }

    // Pass 2: emit translated instructions, patching jumps via new_pc_for.
    let mut out: Vec<Inst> = Vec::with_capacity(new_pc as usize);
    let mut out_lines: Vec<u32> = Vec::with_capacity(new_pc as usize);
    let mut closure_idx2 = 0usize;
    let mut i = 0usize;
    while i < raw_code.len() {
        let inst = raw_code[i];
        let line = raw_lines.get(i).copied().unwrap_or(0);

        // Helper: translate an upvalue index from PUC 5.1's
        // (0..nups) numbering to luna's (env_shift..env_shift+nups).
        let up = |raw_idx: u32| -> Result<u32, String> {
            let shifted = raw_idx + env_shift as u32;
            if shifted > 0xFF {
                return Err(format!("upvalue index {shifted} > 255 after _ENV synth"));
            }
            Ok(shifted)
        };

        // Helper: translate an RK encoding (top bit of 9-bit field set =
        // const-pool index in low 8 bits; clear = register). Returns
        // `(value, is_k)` where `value` is the actual reg or const index
        // fitting in luna's 8-bit field.
        let rk = |raw_field: u32| -> Result<(u32, bool), String> {
            if raw_field & PRE51_BITRK != 0 {
                let k_idx = raw_field & 0xFF;
                if k_idx as usize >= n_consts {
                    return Err(format!("RK const index {k_idx} out of range"));
                }
                Ok((k_idx, true))
            } else {
                if raw_field > 0xFF {
                    return Err(format!("register index {raw_field} > 255"));
                }
                Ok((raw_field, false))
            }
        };

        match inst.op {
            OP_MOVE => {
                out.push(Inst::iabc(Op::Move, inst.a, inst.b, 0, false));
            }
            OP_LOADK => {
                if inst.bx > crate::vm::isa::MAX_BX {
                    return Err(format!("LOADK Bx {} exceeds luna MAX_BX", inst.bx));
                }
                out.push(Inst::iabx(Op::LoadK, inst.a, inst.bx));
            }
            OP_LOADBOOL => {
                // punt-D: only the non-skipping form is supported.
                if inst.c != 0 {
                    return Err("OP_LOADBOOL skip form (C != 0) not yet supported".into());
                }
                let op = if inst.b != 0 {
                    Op::LoadTrue
                } else {
                    Op::LoadFalse
                };
                out.push(Inst::iabc(op, inst.a, 0, 0, false));
            }
            OP_LOADNIL => {
                // 5.1 semantics: R(A..B) := nil  (inclusive range, B counts
                // registers from A *to* B, so the run length is B-A+1).
                // luna's LoadNil uses (A, B) where R(A..A+B) := nil so the
                // luna B = (5.1 B) - A.
                if inst.b < inst.a {
                    return Err(format!(
                        "LOADNIL A={} > B={} (illegal 5.1 range)",
                        inst.a, inst.b
                    ));
                }
                let count_minus_1 = inst.b - inst.a;
                out.push(Inst::iabc(Op::LoadNil, inst.a, count_minus_1, 0, false));
            }
            OP_GETUPVAL => {
                let b = up(inst.b)?;
                out.push(Inst::iabc(Op::GetUpval, inst.a, b, 0, false));
            }
            OP_SETUPVAL => {
                let b = up(inst.b)?;
                out.push(Inst::iabc(Op::SetUpval, inst.a, b, 0, false));
            }
            OP_GETGLOBAL => {
                // GETGLOBAL A Bx → GetTabUp(A, env_idx, Bx) with Bx as
                // const index. luna's GetTabUp packs C as 8 bits; if Bx
                // overflows we'd need an `ExtraArg` chain — not yet
                // supported.
                let env_idx = 0u32; // synth_env put it at slot 0
                if inst.bx > 0xFF {
                    return Err(format!(
                        "GETGLOBAL Bx {} > 255 (ExtraArg unsupported)",
                        inst.bx
                    ));
                }
                out.push(Inst::iabc(Op::GetTabUp, inst.a, env_idx, inst.bx, false));
            }
            OP_SETGLOBAL => {
                let env_idx = 0u32;
                if inst.bx > 0xFF {
                    return Err(format!(
                        "SETGLOBAL Bx {} > 255 (ExtraArg unsupported)",
                        inst.bx
                    ));
                }
                out.push(Inst::iabc(Op::SetTabUp, env_idx, inst.bx, inst.a, false));
            }
            OP_GETTABLE => {
                let (c_val, c_is_k) = rk(inst.c)?;
                let op = if c_is_k { Op::GetField } else { Op::GetTable };
                out.push(Inst::iabc(op, inst.a, inst.b, c_val, c_is_k));
            }
            OP_SETTABLE => {
                let (b_val, b_is_k) = rk(inst.b)?;
                let (c_val, c_is_k) = rk(inst.c)?;
                let op = if b_is_k { Op::SetField } else { Op::SetTable };
                out.push(Inst::iabc(op, inst.a, b_val, c_val, c_is_k));
            }
            OP_NEWTABLE => {
                // punt-C: fb-byte decode — we use the raw bytes as a hint,
                // saturating at 255. Hints are advisory; mis-sizing wastes
                // some realloc time but doesn't affect correctness.
                let b = fb2int_saturating(inst.b);
                let c = fb2int_saturating(inst.c);
                out.push(Inst::iabc(Op::NewTable, inst.a, b, c, false));
            }
            OP_SELF => {
                let (c_val, c_is_k) = rk(inst.c)?;
                out.push(Inst::iabc(Op::SelfOp, inst.a, inst.b, c_val, c_is_k));
            }
            // arith / compare ops — straight RK re-encode
            OP_ADD => arith(&mut out, Op::Add, inst, &rk)?,
            OP_SUB => arith(&mut out, Op::Sub, inst, &rk)?,
            OP_MUL => arith(&mut out, Op::Mul, inst, &rk)?,
            OP_DIV => arith(&mut out, Op::Div, inst, &rk)?,
            OP_MOD => arith(&mut out, Op::Mod, inst, &rk)?,
            OP_POW => arith(&mut out, Op::Pow, inst, &rk)?,
            OP_UNM => {
                out.push(Inst::iabc(Op::Unm, inst.a, inst.b, 0, false));
            }
            OP_NOT => {
                out.push(Inst::iabc(Op::Not, inst.a, inst.b, 0, false));
            }
            OP_LEN => {
                out.push(Inst::iabc(Op::Len, inst.a, inst.b, 0, false));
            }
            OP_CONCAT => {
                // 5.1 CONCAT semantics: R(A) := R(B) .. R(B+1) .. ... .. R(C)
                // luna's Concat: R(A) := R(A) .. R(A+1) .. ... .. R(A+B-1)
                // Mismatch: 5.1 uses B,C inclusive; luna uses A and a count.
                // Translation: when B == A, count = C - A + 1 = C - B + 1.
                if inst.b != inst.a {
                    return Err(format!(
                        "OP_CONCAT B={} ≠ A={} (5.1→luna concat requires B==A)",
                        inst.b, inst.a
                    ));
                }
                if inst.c < inst.b {
                    return Err(format!("OP_CONCAT C={} < B={} (illegal)", inst.c, inst.b));
                }
                let count = inst.c - inst.b + 1;
                out.push(Inst::iabc(Op::Concat, inst.a, count, 0, false));
            }
            OP_JMP => {
                let target_old = (i as i64) + 1 + inst.sbx as i64;
                let target_new = resolve_jump_target(&new_pc_for, target_old)?;
                let delta = target_new - (out.len() as i64 + 1);
                if !(-crate::vm::isa::MAX_SJ as i64..=crate::vm::isa::MAX_SJ as i64)
                    .contains(&delta)
                {
                    return Err(format!("JMP delta {delta} exceeds luna sJ range"));
                }
                out.push(Inst::isj(Op::Jmp, delta as i32));
            }
            OP_EQ => compare(&mut out, Op::Eq, inst, &rk)?,
            OP_LT => compare(&mut out, Op::Lt, inst, &rk)?,
            OP_LE => compare(&mut out, Op::Le, inst, &rk)?,
            OP_TEST => {
                // 5.1 TEST A C: if not (R(A) <=> C) then pc++
                // luna Test A k:   if (not R(A)) == k then pc++
                // Equivalent when k = !C ⇒ luna_k = (C == 0).
                let k = inst.c == 0;
                out.push(Inst::iabc(Op::Test, inst.a, 0, 0, k));
            }
            OP_TESTSET => {
                let k = inst.c == 0;
                out.push(Inst::iabc(Op::TestSet, inst.a, inst.b, 0, k));
            }
            OP_CALL => {
                out.push(Inst::iabc(Op::Call, inst.a, inst.b, inst.c, false));
            }
            OP_TAILCALL => {
                out.push(Inst::iabc(Op::TailCall, inst.a, inst.b, inst.c, false));
            }
            OP_RETURN => {
                out.push(Inst::iabc(Op::Return, inst.a, inst.b, 0, false));
            }
            OP_FORLOOP => {
                let target_old = (i as i64) + 1 + inst.sbx as i64;
                let target_new = resolve_jump_target(&new_pc_for, target_old)?;
                let delta = target_new - (out.len() as i64 + 1);
                // luna's ForLoop uses a signed sBx field — same shape as
                // 5.1, just different bias. Re-pack.
                if !((-crate::vm::isa::MAX_SBX as i64)..=(crate::vm::isa::MAX_SBX as i64))
                    .contains(&delta)
                {
                    return Err(format!("FORLOOP delta {delta} exceeds luna sBx range"));
                }
                out.push(Inst::iasbx(Op::ForLoop, inst.a, delta as i32));
            }
            OP_FORPREP => {
                let target_old = (i as i64) + 1 + inst.sbx as i64;
                let target_new = resolve_jump_target(&new_pc_for, target_old)?;
                let delta = target_new - (out.len() as i64 + 1);
                if !((-crate::vm::isa::MAX_SBX as i64)..=(crate::vm::isa::MAX_SBX as i64))
                    .contains(&delta)
                {
                    return Err(format!("FORPREP delta {delta} exceeds luna sBx range"));
                }
                out.push(Inst::iasbx(Op::ForPrep, inst.a, delta as i32));
            }
            OP_TFORLOOP => {
                // punt-A: 5.1's combined TFORLOOP needs splitting into
                // luna's TForPrep + TForCall + TForLoop. Not yet
                // implemented; generic-for loops will fail to load.
                return Err(
                    "OP_TFORLOOP translation not yet implemented (punt-A — see module docs)".into(),
                );
            }
            OP_SETLIST => {
                // 5.1 SETLIST A B C: R(A)[(C-1)*FPF + i] := R(A+i) for i in 1..B.
                // If C == 0, the next instruction is a literal int with the
                // block index (not an opcode — purely a data word). We
                // reject that case for now; common cases use a fixed C.
                if inst.c == 0 {
                    return Err("OP_SETLIST C=0 (next-inst block index) not yet supported".into());
                }
                if inst.c > 0xFF {
                    return Err(format!("OP_SETLIST C={} > 255", inst.c));
                }
                out.push(Inst::iabc(Op::SetList, inst.a, inst.b, inst.c, false));
            }
            OP_CLOSE => {
                out.push(Inst::iabc(Op::Close, inst.a, 0, 0, false));
            }
            OP_CLOSURE => {
                if inst.bx as usize >= protos.len() {
                    return Err(format!(
                        "OP_CLOSURE proto index {} out of range (have {})",
                        inst.bx,
                        protos.len()
                    ));
                }
                // The corresponding pseudo-instructions follow at i+1..i+1+nups.
                if closure_idx2 >= child_nups.len() {
                    return Err("CLOSURE/pseudo count mismatch (pass 2)".into());
                }
                let n = child_nups[closure_idx2];
                closure_idx2 += 1;
                // Convert each pseudo-instruction into the nested proto's
                // upvalue descriptor. The nested proto's upval list is
                // mutated in place via its `Gc<Proto>` (NonNull<Proto>);
                // SAFETY: we just constructed the Proto and hold the only
                // reference path so it's exclusive.
                let child = protos[inst.bx as usize];
                // SAFETY: see above; alternatively, this could be done
                // before adopting the proto into the heap, but recursion
                // ordering makes that awkward. The Proto is single-threaded
                // and not yet observable to the GC root scanner during
                // load.
                let child_upvals = unsafe { &mut child.as_ptr().as_mut().unwrap().upvals };
                // Skip the synthesised _ENV slot (always at index 0 if
                // the child synthesised one) when filling pseudo-derived
                // descriptors. We detect this via name == "_ENV" at slot 0.
                let child_env_shift: u8 =
                    if !child_upvals.is_empty() && &*child_upvals[0].name == "_ENV" {
                        1
                    } else {
                        0
                    };
                if child_upvals.len() < child_env_shift as usize + n {
                    return Err(format!(
                        "child upval slots {} < env_shift {} + pseudo {}",
                        child_upvals.len(),
                        child_env_shift,
                        n
                    ));
                }
                for j in 0..n {
                    let pseudo = raw_code[i + 1 + j];
                    let (in_stack, src_idx) = match pseudo.op {
                        OP_MOVE => (true, pseudo.b),
                        OP_GETUPVAL => {
                            // Reference into the PARENT's upvalue list;
                            // since the parent shifted everything by
                            // env_shift, shift the index too.
                            (false, pseudo.b + env_shift as u32)
                        }
                        other => {
                            return Err(format!(
                                "OP_CLOSURE pseudo-instruction must be MOVE/GETUPVAL, got op {other}"
                            ));
                        }
                    };
                    if src_idx > 0xFF {
                        return Err(format!("pseudo upval index {src_idx} > 255"));
                    }
                    let slot = child_env_shift as usize + j;
                    // Preserve the name if upvalnames already populated it
                    // (the recursion order: we recursed into the child
                    // BEFORE this loop runs, so r_proto already filled
                    // upvalue names from the upvalnames section).
                    let existing_name = std::mem::take(&mut child_upvals[slot].name);
                    child_upvals[slot] = UpvalDesc {
                        in_stack,
                        index: src_idx as u8,
                        name: existing_name,
                        read_only: false,
                    };
                }
                // Recompute child.env_upval_idx now that descriptors are
                // final (only matters if the child synthesised an _ENV —
                // index 0 — which is already what env_upval_idx was set
                // to in r_proto).
                out.push(Inst::iabx(Op::Closure, inst.a, inst.bx));
                // Skip the pseudo-instructions.
                i += n;
            }
            OP_VARARG => {
                out.push(Inst::iabc(Op::Vararg, inst.a, inst.b, 0, false));
            }
            other => {
                return Err(format!("unsupported PUC 5.1 op {other}"));
            }
        }
        out_lines.push(line);
        i += 1;
    }

    // Sanity: emitted-len must match what pass 1 predicted.
    debug_assert_eq!(out.len() as i64, new_pc, "pass-1 / pass-2 length disagree");
    // Suppress unused warning when assertions compile out.
    let _ = upvals;
    Ok((out, out_lines))
}

fn arith<F>(out: &mut Vec<Inst>, op: Op, inst: Pre51Inst, rk: &F) -> Result<(), String>
where
    F: Fn(u32) -> Result<(u32, bool), String>,
{
    // 5.1 arithmetic: A B C with RK on B and C. luna's Add/Sub/...
    // packs as `R[A] := R[B] + R[C]/K[C]` (k flag on C only — the B
    // side is always a register in luna's binop format). When PUC has K
    // on B side, we need a temp register or to flip operands; for now
    // we only support RK on C — if B is K, lower to `LoadK tmp ; op A tmp C`.
    let (b_val, b_is_k) = rk(inst.b)?;
    let (c_val, c_is_k) = rk(inst.c)?;
    if b_is_k {
        // Lower: LoadK tmp_reg, b_val ; op A tmp_reg C
        // tmp_reg = inst.a (safe IFF A is a fresh dest and we don't read
        // it; for binop with RK-B we conservatively bail and report).
        return Err(format!(
            "arith op with K on B-side (5.1 RK(B)) not yet supported \
             (op={op:?}, A={}, K[{b_val}], …)",
            inst.a
        ));
    }
    out.push(Inst::iabc(op, inst.a, b_val, c_val, c_is_k));
    Ok(())
}

fn compare<F>(out: &mut Vec<Inst>, op: Op, inst: Pre51Inst, rk: &F) -> Result<(), String>
where
    F: Fn(u32) -> Result<(u32, bool), String>,
{
    // 5.1 EQ A B C: if (RK(B) == RK(C)) ~= A then pc++
    // luna Eq A B k: same skip semantics with A=lhs reg, B=rhs reg, k=cond.
    // The 5.1 A is purely a 0/1 flag (cond). luna's Eq uses k as the flag.
    let (b_val, b_is_k) = rk(inst.b)?;
    let (c_val, c_is_k) = rk(inst.c)?;
    // luna's Eq encoding: A is lhs reg (or k-pool index if k-form), B is
    // rhs reg (or k-pool), `k` flag = expected truthiness (skip when
    // result != k).
    if b_is_k || c_is_k {
        return Err(
            "5.1 EQ/LT/LE with RK on operand not yet supported (only register form)".into(),
        );
    }
    // 5.1 EQ: skip when (B==C) != A. luna Eq: skip when (R[A]==R[B]) != k.
    // So map: luna A := 5.1 B, luna B := 5.1 C, luna k := 5.1 A != 0.
    let k = inst.a != 0;
    out.push(Inst::iabc(op, b_val, c_val, 0, k));
    Ok(())
}

/// Look up a translated PC, rejecting jumps that land on a stripped
/// pseudo-instruction (sentinel -1) or out of range. The PUC compiler
/// never emits such jumps, but the check keeps a malformed chunk from
/// constructing a bogus Proto.
fn resolve_jump_target(new_pc_for: &[i64], target_old: i64) -> Result<i64, String> {
    if target_old < 0 || target_old as usize >= new_pc_for.len() {
        // Jumping one past the last instruction is legal in PUC (loop
        // exit); treat that as `out.len()` (== new_pc at end).
        if target_old >= 0 && target_old as usize == new_pc_for.len() {
            return Ok(*new_pc_for.last().unwrap_or(&0) + 1);
        }
        return Err(format!("jump target {target_old} out of range"));
    }
    let v = new_pc_for[target_old as usize];
    if v < 0 {
        return Err(format!(
            "jump target {target_old} lands on stripped pseudo-instruction"
        ));
    }
    Ok(v)
}

/// PUC `luaO_fb2int` — convert an 8-bit floating-byte (eeeeexxx where
/// the value is `(1xxx) << e` for `e > 0`, else just `xxx`) back into an
/// integer. Saturates to `u8::MAX` to fit luna's 8-bit hint field.
fn fb2int_saturating(fb: u32) -> u32 {
    let e = (fb >> 3) & 0x1F;
    let x = fb & 0x07;
    let v = if e == 0 { x } else { (x | 0x08) << (e - 1) };
    v.min(0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fb2int_basic() {
        assert_eq!(fb2int_saturating(0), 0);
        assert_eq!(fb2int_saturating(7), 7);
        assert_eq!(fb2int_saturating(8), 8); // e=1, x=0 → 0x08 << 0
        assert_eq!(fb2int_saturating(0b0000_1111), 15); // e=1, x=7 → 0xF << 0
    }

    #[test]
    fn decode_inst_51_fields() {
        // OP_MOVE (0) A=3 B=5 C=0 → bits: op:6=0, a:8=3 at off 6, c:9=0 at off 14, b:9=5 at off 23
        let raw: u32 = (3u32 << 6) | (5u32 << 23);
        let i = decode_inst_51(raw);
        assert_eq!(i.op, 0);
        assert_eq!(i.a, 3);
        assert_eq!(i.b, 5);
        assert_eq!(i.c, 0);
    }
}
