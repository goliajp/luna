//! PUC Lua 5.1 `.luac` → luna `Proto` translator.
//!
//! Semantics follow `lua-5.1.5/src/lvm.c`; the shared lowering rules live in
//! [`super::lower`]. What is particular to 5.1:
//!
//! 1. **Instruction layout** — `op:6 | A:8 | C:9 | B:9`; `Bx` is `C|B`
//!    (18 bits). A 9-bit B/C with its top bit set is an RK constant.
//!
//! 2. **Globals** — 5.1 reads globals through the function environment
//!    (`GETGLOBAL` / `SETGLOBAL`); luna models that environment as an `_ENV`
//!    upvalue. Like luna's own 5.1 compiler, every translated function gets
//!    `_ENV` as upvalue 0, chained from its parent's, so `setfenv` has a cell
//!    to rewrite even in a function that never touches a global. PUC's own
//!    upvalues follow at 1…
//!
//! 3. **`CLOSURE` pseudo-instructions** — each `CLOSURE` is followed by one
//!    `MOVE 0 B` (capture register B) or `GETUPVAL 0 B` (capture upvalue B)
//!    per upvalue of the new function. They become the child's upvalue
//!    descriptors and emit no code.
//!
//! 4. **Generic `for`** — `TFORLOOP A C` both calls the iterator and tests
//!    its first result; the `JMP` after it jumps back to the body. The pair
//!    becomes luna's `TForCall` + `TForLoop`, inside a loop window (5.1 has
//!    three hidden slots where luna has four).
//!
//! 5. **`SETLIST`** — `C` is a 1-based block number of 50 fields; `C = 0`
//!    takes the block number from the next code word, a raw integer.

use super::lower::{self, Jump, Lowered, Lowering, RawLocVar, RawProto, Window};
use crate::runtime::Value;
use crate::runtime::function::{Proto, UpvalDesc};
use crate::runtime::heap::{Gc, Heap};
use crate::vm::dump::reader::Reader;
use crate::vm::isa::{Inst, Op};

const DIALECT: &str = "PUC 5.1";

/// The C types whose sizes header bytes 7..=10 record.
const SIZE_NAMES: [&str; 4] = [
    "sizeof(int)",
    "sizeof(size_t)",
    "sizeof(Instruction)",
    "sizeof(lua_Number)",
];

/// Header: signature, version, format, endianness, then the sizes of
/// `int`, `size_t`, `Instruction`, `lua_Number`, and the integral flag.
const HEADER: &[u8] = &[0x1b, b'L', b'u', b'a', 0x51, 0, 1, 4, 8, 4, 8, 0];

/// `LFIELDS_PER_FLUSH` (lopcodes.h).
const FIELDS_PER_FLUSH: u64 = 50;

#[derive(Clone, Copy)]
struct I51 {
    op: u8,
    a: u32,
    b: u32,
    c: u32,
}

impl I51 {
    fn decode(w: u32) -> I51 {
        I51 {
            op: (w & 0x3F) as u8,
            a: (w >> 6) & 0xFF,
            c: (w >> 14) & 0x1FF,
            b: (w >> 23) & 0x1FF,
        }
    }
    fn bx(self) -> u32 {
        (self.b << 9) | self.c
    }
    fn sbx(self) -> i64 {
        self.bx() as i64 - 131071
    }
}

// Opcode numbers, lopcodes.h 5.1.5.
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

/// The `_ENV` cell every translated function carries at upvalue 0.
const ENV_UPVAL: u32 = 0;

pub(in crate::vm::dump) fn undump(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    check_header(bytes)?;
    let mut r = Reader::at(bytes, HEADER.len());
    let raw = read_proto(&mut r, heap, None)?;
    if r.pos() != bytes.len() {
        return Err(format!(
            "{DIALECT} chunk: {} trailing bytes",
            bytes.len() - r.pos()
        ));
    }
    lower::build(heap, raw, &translate)
}

fn check_header(bytes: &[u8]) -> Result<(), String> {
    let Some(h) = bytes.get(..HEADER.len()) else {
        return Err(format!("{DIALECT} chunk: truncated header"));
    };
    if h == HEADER {
        return Ok(());
    }
    Err(match h.iter().zip(HEADER).position(|(a, b)| a != b) {
        Some(5) => format!("{DIALECT} chunk: unsupported format byte 0x{:02x}", h[5]),
        Some(6) => format!("{DIALECT} chunk: only little-endian chunks load"),
        Some(11) => format!("{DIALECT} chunk: integral lua_Number builds are not supported"),
        Some(i @ 7..=10) => format!(
            "{DIALECT} chunk: {} is {}, luna needs {}",
            SIZE_NAMES[i - 7],
            h[i],
            HEADER[i]
        ),
        Some(_) => format!("{DIALECT} chunk: bad signature or version"),
        None => unreachable!("slices differ"),
    })
}

fn read_int(r: &mut Reader) -> Result<u32, String> {
    let v = i32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"));
    u32::try_from(v).map_err(|_| format!("{DIALECT} chunk: negative count {v}"))
}

/// An element count, checked against the bytes left (see `Reader::count`).
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, String> {
    let n = read_int(r)?;
    r.count(n as u64, min_size)
}

/// `size_t` length then the bytes, including a trailing NUL that luna drops.
/// Length 0 is PUC's NULL string.
fn read_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, String> {
    let n = u64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"));
    if n == 0 {
        return Ok(None);
    }
    let n = r.count(n, 1)?;
    let s = r.take(n)?;
    Ok(Some(&s[..n - 1]))
}

fn read_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(r.u8()? != 0),
        3 => Value::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        4 => Value::Str(heap.intern(read_string(r)?.unwrap_or(b""))),
        t => return Err(format!("{DIALECT} chunk: bad constant tag {t}")),
    })
}

/// `LoadFunction` order: source, line_defined, last_line_defined, nups,
/// numparams, is_vararg, maxstacksize, code, constants, protos, lineinfo,
/// locvars, upvalue names.
fn read_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::string::LuaStr>>,
) -> Result<RawProto, String> {
    // A nested function's source is dumped as NULL when it equals its
    // parent's.
    let source = match (read_string(r)?, parent_source) {
        (Some(s), _) => heap.intern(s),
        (None, Some(p)) => p,
        (None, None) => heap.intern(b"=?"),
    };
    let line_defined = read_int(r)?;
    let last_line_defined = read_int(r)?;
    let nups = r.u8()? as usize;
    let num_params = r.u8()?;
    // VARARG_ISVARARG = 2, VARARG_NEEDSARG = 4 (lobject.h)
    let vararg = r.u8()?;
    let max_stack = r.u8()?;

    let n = read_count(r, 4)?;
    let code = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 1)?;
    let consts = (0..n)
        .map(|_| read_const(r, heap))
        .collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 1)?;
    let protos = (0..n)
        .map(|_| read_proto(r, heap, Some(source)))
        .collect::<Result<Vec<_>, _>>()?;

    let n = read_count(r, 4)?;
    let lines = (0..n).map(|_| read_int(r)).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 16)?;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b""));
        locvars.push(RawLocVar {
            name: name.into(),
            start_pc: read_int(r)?,
            end_pc: read_int(r)?,
        });
    }
    // Upvalue 0 is the synthesised `_ENV`; the rest are filled in from the
    // parent's pseudo-instructions (a main chunk has none).
    let mut upvals = vec![env_upval()];
    let n = read_count(r, 8)?;
    if n != 0 && n != nups {
        return Err(format!(
            "{DIALECT} chunk: {n} upvalue names for {nups} upvalues"
        ));
    }
    for i in 0..nups {
        let name = if i < n {
            String::from_utf8_lossy(read_string(r)?.unwrap_or(b"")).into()
        } else {
            "".into()
        };
        upvals.push(UpvalDesc {
            in_stack: false,
            index: 0,
            name,
            read_only: false,
        });
    }

    Ok(RawProto {
        source,
        line_defined,
        last_line_defined,
        num_params,
        is_vararg: vararg & 2 != 0,
        has_compat_vararg_arg: vararg & 4 != 0,
        vararg_table: false,
        max_stack,
        code,
        consts,
        upvals,
        protos,
        lines,
        locvars,
    })
}

/// A function's `_ENV`: for the main chunk `Vm::load` seeds it with the
/// globals table; a nested function captures its parent's upvalue 0.
fn env_upval() -> UpvalDesc {
    UpvalDesc {
        in_stack: false,
        index: ENV_UPVAL as u8,
        name: "_ENV".into(),
        read_only: false,
    }
}

/// `TFORLOOP A C` at `p` with its back-`JMP` at `p+1`: the body runs from the
/// jump's target to `p+1`, and the loop variables start at `A+3`.
fn loop_windows(code: &[u32]) -> Result<Vec<Window>, String> {
    let mut out = Vec::new();
    for (p, &w) in code.iter().enumerate() {
        let i = I51::decode(w);
        if i.op != OP_TFORLOOP {
            continue;
        }
        let jmp = code.get(p + 1).map(|&w| I51::decode(w));
        let Some(jmp) = jmp.filter(|j| j.op == OP_JMP) else {
            return Err(format!(
                "{DIALECT} chunk: TFORLOOP without its back-jump (pc {p})"
            ));
        };
        let body = p as i64 + 2 + jmp.sbx();
        if !(0..=p as i64).contains(&body) {
            return Err(format!(
                "{DIALECT} chunk: TFORLOOP back-jump to {body} (pc {p})"
            ));
        }
        out.push(Window {
            first: body as usize,
            last: p + 1,
            pivot: i.a + 3,
        });
    }
    Ok(out)
}

fn translate(raw: &mut RawProto) -> Result<Lowered, String> {
    let windows = loop_windows(&raw.code)?;
    let mut lw = Lowering::new(DIALECT, raw.code.len(), raw.max_stack, windows);
    let mut closed = vec![false; raw.protos.len()];
    let code = &raw.code;
    let mut pc = 0;
    while pc < code.len() {
        lw.begin(pc, raw.lines.get(pc).copied().unwrap_or(0));
        let i = I51::decode(code[pc]);
        let next = pc as i64 + 1;
        match i.op {
            OP_MOVE => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(Op::Move, a, b, 0, false));
            }
            OP_LOADK => {
                let a = lw.r(i.a)?;
                lw.load_k(a, i.bx())?;
            }
            OP_LOADBOOL => {
                let a = lw.r(i.a)?;
                match (i.b != 0, i.c != 0) {
                    (false, false) => lw.emit(Inst::iabc(Op::LoadFalse, a, 0, 0, false)),
                    (false, true) => lw.emit(Inst::iabc(Op::LFalseSkip, a, 0, 0, false)),
                    (true, false) => lw.emit(Inst::iabc(Op::LoadTrue, a, 0, 0, false)),
                    (true, true) => {
                        lw.emit(Inst::iabc(Op::LoadTrue, a, 0, 0, false));
                        lw.jump(Inst::isj(Op::Jmp, 0), Jump::Jmp, next + 1)?;
                    }
                }
            }
            OP_LOADNIL => {
                // R(A..=B) := nil
                if i.b < i.a {
                    return Err(lw.err(format_args!("LOADNIL {}..{}", i.a, i.b)));
                }
                let a = lw.run(i.a, i.b - i.a + 1)?;
                lw.emit(Inst::iabc(Op::LoadNil, a, i.b - i.a, 0, false));
            }
            OP_GETUPVAL => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::GetUpval, a, i.b + 1, 0, false));
            }
            OP_SETUPVAL => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::SetUpval, a, i.b + 1, 0, false));
            }
            OP_GETGLOBAL => {
                let a = lw.r(i.a)?;
                lw.get_tabup(a, ENV_UPVAL, i.bx(), true)?;
            }
            OP_SETGLOBAL => {
                let a = lw.r(i.a)?;
                lw.set_tabup(ENV_UPVAL, i.bx(), a, true)?;
            }
            OP_GETTABLE => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.get_table_rk(a, b, i.c)?;
            }
            OP_SETTABLE => {
                let a = lw.r(i.a)?;
                lw.set_table_rk(a, i.b, i.c)?;
            }
            // luna's NewTable ignores its size hints.
            OP_NEWTABLE => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::NewTable, a, 0, 0, false));
            }
            OP_SELF => lw.self_rk(i.a, i.b, i.c)?,
            OP_ADD | OP_SUB | OP_MUL | OP_DIV | OP_MOD | OP_POW => {
                let op = match i.op {
                    OP_ADD => Op::Add,
                    OP_SUB => Op::Sub,
                    OP_MUL => Op::Mul,
                    OP_DIV => Op::Div,
                    OP_MOD => Op::Mod,
                    _ => Op::Pow,
                };
                let a = lw.r(i.a)?;
                let b = lw.rk(i.b)?;
                let c = lw.rk(i.c)?;
                lw.emit(Inst::iabc(op, a, b, c, false));
            }
            OP_UNM | OP_NOT | OP_LEN => {
                let op = match i.op {
                    OP_UNM => Op::Unm,
                    OP_NOT => Op::Not,
                    _ => Op::Len,
                };
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(op, a, b, 0, false));
            }
            OP_CONCAT => lw.concat_range(i.a, i.b, i.c)?,
            OP_JMP => lw.jump(Inst::isj(Op::Jmp, 0), Jump::Jmp, next + i.sbx())?,
            OP_EQ => lw.compare_rk(Op::Eq, i.a != 0, i.b, i.c)?,
            OP_LT => lw.compare_rk(Op::Lt, i.a != 0, i.b, i.c)?,
            OP_LE => lw.compare_rk(Op::Le, i.a != 0, i.b, i.c)?,
            // TEST A C: if not (R(A) <=> C) then pc++
            OP_TEST => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::Test, a, 0, 0, i.c != 0));
            }
            // TESTSET A B C: if (R(B) <=> C) then R(A) := R(B) else pc++
            OP_TESTSET => {
                let (a, b) = (lw.r(i.a)?, lw.r(i.b)?);
                lw.emit(Inst::iabc(Op::TestSet, a, b, 0, i.c != 0));
            }
            OP_CALL => {
                let (b, c) = (lw.byte(i.b, "CALL B")?, lw.byte(i.c, "CALL C")?);
                let a = lw.run(i.a, b.max(c.saturating_sub(1)).max(1))?;
                lw.emit(Inst::iabc(Op::Call, a, b, c, false));
            }
            OP_TAILCALL => {
                let b = lw.byte(i.b, "TAILCALL B")?;
                let a = lw.run(i.a, b.max(1))?;
                lw.emit(Inst::iabc(Op::TailCall, a, b, 0, false));
            }
            OP_RETURN => lw.ret(i.a, i.b)?,
            // FORPREP jumps to its FORLOOP; FORLOOP jumps back to the body.
            OP_FORPREP => {
                let a = lw.run(i.a, 4)?;
                lw.jump(Inst::iabx(Op::ForPrep, a, 0), Jump::ForPrep, next + i.sbx())?;
            }
            OP_FORLOOP => {
                let a = lw.run(i.a, 4)?;
                lw.jump(Inst::iabx(Op::ForLoop, a, 0), Jump::Back, next + i.sbx())?;
            }
            OP_TFORLOOP => {
                // `loop_windows` checked that a JMP follows.
                let c = lw.byte(i.c, "TFORLOOP C")?;
                let a = lw.run(i.a, 3)?;
                lw.run(i.a + 3, c.max(1))?;
                lw.emit(Inst::iabc(Op::TForCall, a, 0, c, false));
                pc += 1;
                let jmp = I51::decode(code[pc]);
                lw.begin(pc, raw.lines.get(pc).copied().unwrap_or(0));
                let body = pc as i64 + 1 + jmp.sbx();
                lw.jump(Inst::iabx(Op::TForLoop, a, 0), Jump::Back, body)?;
            }
            OP_SETLIST => {
                let block = if i.c == 0 {
                    pc += 1;
                    *code
                        .get(pc)
                        .ok_or_else(|| lw.err("SETLIST without its block word"))?
                } else {
                    i.c
                };
                if block == 0 {
                    return Err(lw.err("SETLIST block number 0"));
                }
                let a = lw.run(i.a, i.b + 1)?;
                lw.set_list(a, i.b, (block as u64 - 1) * FIELDS_PER_FLUSH)?;
            }
            OP_CLOSE => {
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabc(Op::Close, a, 0, 0, false));
            }
            OP_CLOSURE => {
                let idx = i.bx() as usize;
                let Some(child) = raw.protos.get_mut(idx) else {
                    return Err(lw.err(format_args!("CLOSURE of missing function {idx}")));
                };
                if std::mem::replace(&mut closed[idx], true) {
                    return Err(lw.err(format_args!("function {idx} instantiated twice")));
                }
                for u in 1..child.upvals.len() {
                    let Some(&w) = code.get(pc + u) else {
                        return Err(lw.err("CLOSURE without its upvalue pseudo-instructions"));
                    };
                    let p = I51::decode(w);
                    let (in_stack, index) = match p.op {
                        OP_MOVE => (true, lw.r(p.b)?),
                        OP_GETUPVAL => (false, p.b + 1),
                        op => {
                            return Err(lw.err(format_args!(
                                "CLOSURE upvalue pseudo-instruction has opcode {op}"
                            )));
                        }
                    };
                    let index = u8::try_from(index)
                        .map_err(|_| lw.err(format_args!("upvalue index {index} past 255")))?;
                    child.upvals[u].in_stack = in_stack;
                    child.upvals[u].index = index;
                }
                let n_pseudo = child.upvals.len() - 1;
                let a = lw.r(i.a)?;
                lw.emit(Inst::iabx(Op::Closure, a, idx as u32));
                pc += n_pseudo;
            }
            OP_VARARG => {
                let b = lw.byte(i.b, "VARARG B")?;
                let a = lw.run(i.a, b.saturating_sub(1).max(1))?;
                lw.emit(Inst::iabc(Op::Vararg, a, 0, b, false));
            }
            op => return Err(lw.err(format_args!("unknown opcode {op}"))),
        }
        pc += 1;
    }
    lw.finish(&raw.locvars)
}
