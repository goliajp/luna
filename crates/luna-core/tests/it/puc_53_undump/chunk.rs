//! Hand-made PUC 5.3 chunks: the header, the instruction encodings and
//! the chunks the tests load.

/// PUC 5.3 binary header — 33 bytes. Mirrors `lundump.h` 5.3.6 +
/// `lua-5.3-src/lua-5.3.6/src/lundump.c::checkHeader`.
pub(super) const HEADER_53: [u8; 33] = [
    0x1b,
    b'L',
    b'u',
    b'a',
    0x53,
    0x00, // signature + version + format
    0x19,
    0x93,
    b'\r',
    b'\n',
    0x1a,
    b'\n',                              // LUAC_DATA
    4,                                  // sizeof(int)
    std::mem::size_of::<usize>() as u8, // sizeof(size_t)
    4,                                  // sizeof(Instruction)
    8,                                  // sizeof(lua_Integer)
    8,                                  // sizeof(lua_Number)
    // LUAC_INT = 0x5678 (LE i64)
    0x78,
    0x56,
    0,
    0,
    0,
    0,
    0,
    0, // LUAC_NUM = 370.5 (LE f64)
    0,
    0,
    0,
    0,
    0,
    0x28,
    0x77,
    0x40,
];

/// Encode a 5.3 iABC instruction in `op:6 | A:8 | C:9 | B:9` layout.
pub(super) fn enc(op: u8, a: u32, b: u32, c: u32) -> u32 {
    debug_assert!(op < 64);
    debug_assert!(a < 256);
    debug_assert!(b < 512);
    debug_assert!(c < 512);
    (op as u32) | (a << 6) | (c << 14) | (b << 23)
}

pub(super) fn enc_bx(op: u8, a: u32, bx: u32) -> u32 {
    debug_assert!(bx < (1 << 18));
    (op as u32) | (a << 6) | (bx << 14)
}

#[allow(dead_code)]
pub(super) fn enc_sbx(op: u8, a: u32, sbx: i32) -> u32 {
    let bx = (sbx + 131071) as u32;
    enc_bx(op, a, bx)
}

/// Code a constant-pool index as a PUC 5.3 RK operand. `BITRK = 1 << 8`.
#[allow(dead_code)]
pub(super) fn rk(idx: u32) -> u32 {
    debug_assert!(idx < 256);
    idx | (1 << 8)
}

/// PUC 5.3 length-prefixed string. byte = (len+1) if < 0xFF, else
/// 0xFF followed by a size_t (the target's, LE) holding (len+1). Empty
/// strings ship as a single `0x00` byte (size==0 → NULL sentinel).
pub(super) fn puc53_str(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + s.len() + 8);
    if s.is_empty() {
        v.push(0);
        return v;
    }
    let sized = s.len() + 1;
    if sized < 0xFF {
        v.push(sized as u8);
    } else {
        v.push(0xFF);
        v.extend_from_slice(&sized.to_le_bytes());
    }
    v.extend_from_slice(s);
    v
}

pub(super) fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
pub(super) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
pub(super) fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
pub(super) fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

// ---- opcode constants — kept in sync with translator's table ----
pub(super) const OP_LOADK: u8 = 1;
pub(super) const OP_LOADBOOL: u8 = 3;
#[allow(dead_code)]
pub(super) const OP_GETTABUP: u8 = 6;
#[allow(dead_code)]
pub(super) const OP_ADD: u8 = 13;
pub(super) const OP_RETURN: u8 = 38;

/// Build a PUC 5.3 main proto whose body is `LOADK R0 K0; RETURN R0 2`
/// where K0 is the integer 42. Designed to exercise:
/// - header check (LUAC_INT, LUAC_NUM, size bytes)
/// - const-pool LUA_TNUMINT decode
/// - LOADK iABx re-encode
/// - RETURN iABC re-encode
pub(super) fn build_return_42_chunk() -> Vec<u8> {
    let mut body = Vec::new();
    // main has 1 upvalue (the _ENV cell that `Vm::load` will fill with
    // the globals table). It carries no name in the upvalue table — the
    // debug section names it "_ENV".
    body.push(1u8); // nupvalues for main closure (read before LoadFunction)

    // ---- main proto ----
    body.extend_from_slice(&puc53_str(b"@test")); // source
    put_i32(&mut body, 0); // linedefined
    put_i32(&mut body, 0); // lastlinedefined
    body.push(0); // numparams
    body.push(1); // is_vararg (1 = VARARG_ISVARARG in 5.3)
    body.push(2); // maxstacksize

    // ---- code (2 insts) ----
    put_i32(&mut body, 2);
    put_u32(&mut body, enc_bx(OP_LOADK, 0, 0)); // LOADK R0 K0
    put_u32(&mut body, enc(OP_RETURN, 0, 2, 0)); // RETURN R0 B=2

    // ---- constants (1 int) ----
    put_i32(&mut body, 1);
    body.push(0x13); // LUA_TNUMINT = 3 | (1 << 4) = 19
    put_i64(&mut body, 42);

    // ---- upvalues (1: _ENV at parent stack idx 0) ----
    put_i32(&mut body, 1);
    body.push(1); // instack = true (main's _ENV captured from globals)
    body.push(0); // idx

    // ---- nested protos (0) ----
    put_i32(&mut body, 0);

    // ---- debug: lineinfo (2 entries) ----
    put_i32(&mut body, 2);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    // locvars (0)
    put_i32(&mut body, 0);
    // upvalue names (1: "_ENV")
    put_i32(&mut body, 1);
    body.extend_from_slice(&puc53_str(b"_ENV"));

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&HEADER_53);
    chunk.extend(body);
    chunk
}

/// Same shape but constant is a float 3.5.
pub(super) fn build_return_float_chunk() -> Vec<u8> {
    let mut body = Vec::new();
    body.push(1u8);
    body.extend_from_slice(&puc53_str(b"@test"));
    put_i32(&mut body, 0);
    put_i32(&mut body, 0);
    body.push(0);
    body.push(1);
    body.push(2);

    put_i32(&mut body, 2);
    put_u32(&mut body, enc_bx(OP_LOADK, 0, 0));
    put_u32(&mut body, enc(OP_RETURN, 0, 2, 0));

    put_i32(&mut body, 1);
    body.push(3); // LUA_TNUMFLT
    put_f64(&mut body, 3.5);

    put_i32(&mut body, 1);
    body.push(1);
    body.push(0);
    put_i32(&mut body, 0);
    put_i32(&mut body, 2);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    put_i32(&mut body, 0);
    put_i32(&mut body, 1);
    body.extend_from_slice(&puc53_str(b"_ENV"));

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&HEADER_53);
    chunk.extend(body);
    chunk
}

// ---- tests ----
