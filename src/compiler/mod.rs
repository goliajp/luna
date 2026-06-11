//! AST → bytecode compiler. Register model follows PUC lparser/lcode:
//! locals pin the low registers, temporaries grow from `freereg`, constants
//! are deduplicated, forward jumps are patch lists (plain Vecs instead of
//! PUC's in-code jump chains).
//!
//! Slice 2 scope: expressions, locals, assignments, control flow, numeric
//! for, table constructors, globals via the chunk's `_ENV` upvalue. Calls,
//! closures, varargs and generic `for` land in slice 3; goto/labels, attribs
//! enforcement and `global` declarations in slice 5.

use std::collections::HashMap;

use crate::frontend::ast::{
    AttribName, BinOp, Block, Chunk, Expr, ExprId, Stat, StatId, TableField, UnOp,
};
use crate::frontend::error::SyntaxError;
use crate::runtime::heap::{GcHeader, ObjTag};
use crate::runtime::{Gc, Heap, Proto, UpvalDesc, Value};
use crate::version::LuaVersion;
use crate::vm::isa::{Inst, MAX_BX, MAX_SJ, Op};

pub fn compile_chunk(
    ast: &Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<Gc<Proto>, SyntaxError> {
    let source = heap.intern(source_name);
    let mut fs = FuncState {
        ast,
        heap,
        version,
        code: Vec::new(),
        lines: Vec::new(),
        consts: Vec::new(),
        const_map: HashMap::new(),
        locals: Vec::new(),
        blocks: Vec::new(),
        freereg: 0,
        max_stack: 2,
        upvals: vec![UpvalDesc {
            in_stack: false,
            index: 0,
            name: "_ENV".into(),
        }],
        last_line: 0,
    };
    fs.enter_block(false);
    fs.stat_block(&ast.block)?;
    fs.leave_block();
    fs.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
    let proto = Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: fs.code.into_boxed_slice(),
        consts: fs.consts.into_boxed_slice(),
        protos: Box::new([]),
        upvals: fs.upvals.into_boxed_slice(),
        num_params: 0,
        is_vararg: true,
        max_stack: fs.max_stack as u8,
        lines: fs.lines.into_boxed_slice(),
        source,
        line_defined: 0,
    };
    Ok(heap.adopt_proto(proto))
}

const MAX_REGS: u32 = 254;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    Float(u64),
    Str(*mut crate::runtime::LuaStr),
}

struct LocalVar {
    name: Box<str>,
    reg: u32,
    read_only: bool,
}

struct BlockCx {
    first_local: usize,
    is_loop: bool,
    breaks: Vec<usize>,
}

/// Where an expression's value currently lives.
enum Exp {
    Nil,
    True,
    False,
    Int(i64),
    Float(f64),
    Const(u32),
    /// value sits in a register (local or materialized temp)
    Reg(u32),
    /// instruction at index has an unassigned A (destination pending)
    Reloc(usize),
    /// comparison not yet materialized: (op-emitter info)
    Cmp {
        op: Op,
        l: u32,
        r: u32,
    },
}

struct FuncState<'a> {
    ast: &'a Chunk,
    heap: &'a mut Heap,
    version: LuaVersion,
    code: Vec<Inst>,
    lines: Vec<u32>,
    consts: Vec<Value>,
    const_map: HashMap<ConstKey, u32>,
    locals: Vec<LocalVar>,
    blocks: Vec<BlockCx>,
    freereg: u32,
    max_stack: u32,
    upvals: Vec<UpvalDesc>,
    last_line: u32,
}

impl<'a> FuncState<'a> {
    // ---- infrastructure ----

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into(),
        }
    }

    fn emit(&mut self, i: Inst) -> usize {
        self.code.push(i);
        self.lines.push(self.last_line);
        self.code.len() - 1
    }

    fn emit_jump(&mut self) -> usize {
        self.emit(Inst::isj(Op::Jmp, 0))
    }

    fn patch_jump(&mut self, pc: usize) -> Result<(), SyntaxError> {
        let target = self.code.len();
        let off = target as i64 - pc as i64 - 1;
        if off.unsigned_abs() > MAX_SJ as u64 {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.code[pc].set_sj(off as i32);
        Ok(())
    }

    fn jump_back(&mut self, target: usize) -> Result<(), SyntaxError> {
        let off = target as i64 - self.code.len() as i64 - 1;
        if off.unsigned_abs() > MAX_SJ as u64 {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.emit(Inst::isj(Op::Jmp, off as i32));
        Ok(())
    }

    fn reserve(&mut self, n: u32) -> Result<u32, SyntaxError> {
        let base = self.freereg;
        self.freereg += n;
        if self.freereg > MAX_REGS {
            return Err(self.err(
                self.last_line,
                "function or expression needs too many registers",
            ));
        }
        if self.freereg > self.max_stack {
            self.max_stack = self.freereg;
        }
        Ok(base)
    }

    fn const_idx(&mut self, key: ConstKey, v: Value) -> u32 {
        if let Some(&i) = self.const_map.get(&key) {
            return i;
        }
        let i = self.consts.len() as u32;
        self.consts.push(v);
        self.const_map.insert(key, i);
        i
    }

    fn str_const(&mut self, bytes: &[u8]) -> u32 {
        let s = self.heap.intern(bytes);
        self.const_idx(ConstKey::Str(s.as_ptr()), Value::Str(s))
    }

    // ---- scopes ----

    fn enter_block(&mut self, is_loop: bool) {
        self.blocks.push(BlockCx {
            first_local: self.locals.len(),
            is_loop,
            breaks: Vec::new(),
        });
    }

    fn leave_block(&mut self) {
        let b = self.blocks.pop().expect("block underflow");
        debug_assert!(b.breaks.is_empty(), "breaks must be patched by the loop");
        self.close_block_locals(b.first_local);
    }

    fn leave_loop_block(&mut self) -> Result<(), SyntaxError> {
        let b = self.blocks.pop().expect("block underflow");
        self.close_block_locals(b.first_local);
        for pc in b.breaks {
            self.patch_jump(pc)?;
        }
        Ok(())
    }

    fn close_block_locals(&mut self, first: usize) {
        if self.locals.len() > first {
            let base = self.locals[first].reg;
            self.locals.truncate(first);
            self.freereg = base;
        }
    }

    fn declare_local(&mut self, name: &str, reg: u32, read_only: bool) {
        self.locals.push(LocalVar {
            name: name.into(),
            reg,
            read_only,
        });
    }

    fn resolve_local(&self, name: &str) -> Option<&LocalVar> {
        self.locals.iter().rev().find(|l| &*l.name == name)
    }

    // ---- expressions ----

    fn expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        match self.ast.expr(id) {
            Expr::Nil => Ok(Exp::Nil),
            Expr::True => Ok(Exp::True),
            Expr::False => Ok(Exp::False),
            Expr::Int(i) => Ok(Exp::Int(*i)),
            Expr::Float(f) => Ok(Exp::Float(*f)),
            Expr::Str(s) => {
                let s = s.clone();
                Ok(Exp::Const(self.str_const(&s)))
            }
            Expr::Name(n) => {
                self.last_line = n.line;
                if let Some(l) = self.resolve_local(&n.text) {
                    return Ok(Exp::Reg(l.reg));
                }
                // global: _ENV.name (upvalue 0 in slice 2)
                let text = n.text.clone();
                self.global_access(&text)
            }
            Expr::Paren(inner) => self.expr(*inner),
            Expr::Index { obj, key } => self.index_expr(*obj, *key),
            Expr::UnOp { op, operand, line } => {
                let (op, operand, line) = (*op, *operand, *line);
                self.unop(op, operand, line)
            }
            Expr::BinOp { op, lhs, rhs, line } => {
                let (op, lhs, rhs, line) = (*op, *lhs, *rhs, *line);
                self.binop(op, lhs, rhs, line)
            }
            Expr::Table { line, .. } => {
                let line = *line;
                self.table_ctor(id, line)
            }
            Expr::Vararg => Err(self.err(
                self.last_line,
                "vararg expressions are not supported yet (P03 slice 3)",
            )),
            Expr::Call { line, .. } | Expr::MethodCall { line, .. } => {
                Err(self.err(*line, "function calls are not supported yet (P03 slice 3)"))
            }
            Expr::Function(body) => Err(self.err(
                body.line,
                "function expressions are not supported yet (P03 slice 3)",
            )),
        }
    }

    fn global_access(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        let c = self.str_const(name.as_bytes());
        if c <= 0xFF {
            Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetTabUp,
                0,
                0,
                c,
                true,
            ))))
        } else {
            // rare: huge constant tables — go through registers
            let r = self.reserve(2)?;
            self.emit(Inst::iabc(Op::GetUpval, r, 0, 0, false));
            self.load_const(r + 1, c);
            self.freereg = r;
            Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetTable,
                0,
                r,
                r + 1,
                false,
            ))))
        }
    }

    fn load_const(&mut self, reg: u32, c: u32) {
        if c <= MAX_BX {
            self.emit(Inst::iabx(Op::LoadK, reg, c));
        } else {
            self.emit(Inst::iabx(Op::LoadKx, reg, 0));
            self.emit(Inst::iax(Op::ExtraArg, c));
        }
    }

    /// Materialize into a specific register.
    fn exp_to_reg(&mut self, e: Exp, reg: u32) -> Result<(), SyntaxError> {
        match e {
            Exp::Nil => {
                self.emit(Inst::iabc(Op::LoadNil, reg, 0, 0, false));
            }
            Exp::True => {
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
            Exp::False => {
                self.emit(Inst::iabc(Op::LoadFalse, reg, 0, 0, false));
            }
            Exp::Int(i) => {
                if (-(MAX_SJ >> 8) as i64..=(MAX_SJ >> 8) as i64).contains(&i) {
                    // fits sBx (17-bit signed)
                    self.emit(Inst::iasbx(Op::LoadI, reg, i as i32));
                } else {
                    let c = self.const_idx(ConstKey::Int(i), Value::Int(i));
                    self.load_const(reg, c);
                }
            }
            Exp::Float(f) => {
                // LOADF carries small integral floats; otherwise constant
                let as_int = f as i32;
                if as_int as f64 == f && (-65535..=65535).contains(&as_int) {
                    self.emit(Inst::iasbx(Op::LoadF, reg, as_int));
                } else {
                    let c = self.const_idx(ConstKey::Float(f.to_bits()), Value::Float(f));
                    self.load_const(reg, c);
                }
            }
            Exp::Const(c) => self.load_const(reg, c),
            Exp::Reg(r) => {
                if r != reg {
                    self.emit(Inst::iabc(Op::Move, reg, r, 0, false));
                }
            }
            Exp::Reloc(pc) => {
                let i = self.code[pc];
                self.code[pc] = Inst(i.0 & !(0xFF << 7) | (reg << 7));
            }
            Exp::Cmp { op, l, r } => {
                // cond ; JMP +1 ; LFALSESKIP ; LOADTRUE
                self.emit(Inst::iabc(op, l, r, 0, true));
                self.emit(Inst::isj(Op::Jmp, 1));
                self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
        }
        Ok(())
    }

    fn exp_to_nextreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    /// Value into some register without forcing a fresh one for locals.
    fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        if let Exp::Reg(r) = e {
            return Ok(r);
        }
        self.exp_to_nextreg(e)
    }

    /// Compile a condition: emit test(s) so that a following JMP (returned
    /// pc) is taken when the condition is FALSE.
    fn cond_jump_false(&mut self, id: ExprId) -> Result<usize, SyntaxError> {
        let e = self.expr(id)?;
        let saved = self.freereg;
        match e {
            Exp::Cmp { op, l, r } => {
                // k=false: VM executes the JMP when cmp result == false
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.freereg = saved;
        Ok(self.emit_jump())
    }

    fn unop(&mut self, op: UnOp, operand: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        // constant folding for numeric negation (PUC folds in lcode)
        let e = self.expr(operand)?;
        let saved = self.freereg;
        let (opcode, folded) = match op {
            UnOp::Neg => match e {
                Exp::Int(i) => return Ok(Exp::Int(i.wrapping_neg())),
                Exp::Float(f) => return Ok(Exp::Float(-f)),
                e => (Op::Unm, e),
            },
            UnOp::Not => match e {
                Exp::Nil | Exp::False => return Ok(Exp::True),
                Exp::True | Exp::Int(_) | Exp::Float(_) | Exp::Const(_) => return Ok(Exp::False),
                e => (Op::Not, e),
            },
            UnOp::Len => (Op::Len, e),
            UnOp::BNot => match e {
                Exp::Int(i) => return Ok(Exp::Int(!i)),
                e => (Op::BNot, e),
            },
        };
        let r = self.exp_to_anyreg(folded)?;
        self.freereg = saved;
        self.last_line = line;
        Ok(Exp::Reloc(self.emit(Inst::iabc(opcode, 0, r, 0, false))))
    }

    fn binop(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        match op {
            BinOp::And | BinOp::Or => return self.and_or(op, lhs, rhs, line),
            BinOp::Concat => return self.concat(lhs, rhs, line),
            _ => {}
        }
        let saved = self.freereg;
        let le = self.expr(lhs)?;
        // numeric constant folding (PUC luaK_foldconsts subset: arith on two
        // numeric literals, except division/modulo by zero edge cases kept
        // for runtime semantics)
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs) {
            return Ok(folded);
        }
        let l = self.exp_to_anyreg(le)?;
        let re = self.expr(rhs)?;
        let r = self.exp_to_anyreg(re)?;
        self.freereg = saved;
        self.last_line = line;
        let e = match op {
            BinOp::Add => self.arith(Op::Add, l, r),
            BinOp::Sub => self.arith(Op::Sub, l, r),
            BinOp::Mul => self.arith(Op::Mul, l, r),
            BinOp::Div => self.arith(Op::Div, l, r),
            BinOp::IDiv => self.arith(Op::IDiv, l, r),
            BinOp::Mod => self.arith(Op::Mod, l, r),
            BinOp::Pow => self.arith(Op::Pow, l, r),
            BinOp::BAnd => self.arith(Op::BAnd, l, r),
            BinOp::BOr => self.arith(Op::BOr, l, r),
            BinOp::BXor => self.arith(Op::BXor, l, r),
            BinOp::Shl => self.arith(Op::Shl, l, r),
            BinOp::Shr => self.arith(Op::Shr, l, r),
            BinOp::Eq => Exp::Cmp { op: Op::Eq, l, r },
            BinOp::Ne => {
                // a ~= b  ⇒  not (a == b): swap the k sense at use sites by
                // materializing through Cmp with swapped emission
                return self.negate_cmp(Exp::Cmp { op: Op::Eq, l, r });
            }
            BinOp::Lt => Exp::Cmp { op: Op::Lt, l, r },
            BinOp::Le => Exp::Cmp { op: Op::Le, l, r },
            BinOp::Gt => Exp::Cmp {
                op: Op::Lt,
                l: r,
                r: l,
            },
            BinOp::Ge => Exp::Cmp {
                op: Op::Le,
                l: r,
                r: l,
            },
            BinOp::And | BinOp::Or | BinOp::Concat => unreachable!(),
        };
        Ok(e)
    }

    fn arith(&mut self, op: Op, l: u32, r: u32) -> Exp {
        Exp::Reloc(self.emit(Inst::iabc(op, 0, l, r, false)))
    }

    /// not(cmp): materialize inverted via k flip at emission time.
    fn negate_cmp(&mut self, e: Exp) -> Result<Exp, SyntaxError> {
        let Exp::Cmp { op, l, r } = e else {
            unreachable!()
        };
        // emit with k=false so the JMP is taken when equal⇒false path gives
        // true; reuse the materialization skeleton with inverted k
        let reg_holder = self.reserve(1)?;
        self.freereg -= 1;
        let reg = reg_holder;
        self.emit(Inst::iabc(op, l, r, 0, false));
        self.emit(Inst::isj(Op::Jmp, 1));
        self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
        self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
        // value lives in reg, but reg is below freereg now: copy semantics
        // are preserved because the consumer immediately materializes
        Ok(Exp::Reg(reg))
    }

    fn and_or(
        &mut self,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    ) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        let le = self.expr(lhs)?;
        let reg = self.reserve(1)?;
        self.exp_to_reg(le, reg)?;
        // TEST with k: JMP taken (short-circuit, keep lhs) when
        //   and: lhs falsy   (k = false)
        //   or:  lhs truthy  (k = true)
        let k = op == BinOp::Or;
        self.emit(Inst::iabc(Op::Test, reg, 0, 0, k));
        let jmp = self.emit_jump();
        let re = self.expr(rhs)?;
        self.exp_to_reg(re, reg)?;
        self.freereg = reg + 1;
        self.patch_jump(jmp)?;
        Ok(Exp::Reg(reg))
    }

    fn concat(&mut self, lhs: ExprId, rhs: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        // operands stacked consecutively; CONCAT A B folds R[A..A+B-1]
        let base = self.freereg;
        let le = self.expr(lhs)?;
        self.exp_to_nextreg(le)?;
        let re = self.expr(rhs)?;
        self.exp_to_nextreg(re)?;
        self.freereg = base;
        self.last_line = line;
        self.emit(Inst::iabc(Op::Concat, base, 2, 0, false));
        Ok(Exp::Reg(base))
    }

    fn index_expr(&mut self, obj: ExprId, key: ExprId) -> Result<Exp, SyntaxError> {
        let saved = self.freereg;
        let oe = self.expr(obj)?;
        let o = self.exp_to_anyreg(oe)?;
        let e = match self.ast.expr(key) {
            Expr::Str(s) if s.len() <= 255 => {
                let s = s.clone();
                let c = self.str_const(&s);
                if c <= 0xFF {
                    Exp::Reloc(self.emit(Inst::iabc(Op::GetField, 0, o, c, true)))
                } else {
                    let ke = Exp::Const(c);
                    let k = self.exp_to_nextreg(ke)?;
                    Exp::Reloc(self.emit(Inst::iabc(Op::GetTable, 0, o, k, false)))
                }
            }
            Expr::Int(i) if (0..=255).contains(i) => {
                let c = *i as u32;
                Exp::Reloc(self.emit(Inst::iabc(Op::GetI, 0, o, c, false)))
            }
            _ => {
                let ke = self.expr(key)?;
                let k = self.exp_to_anyreg(ke)?;
                Exp::Reloc(self.emit(Inst::iabc(Op::GetTable, 0, o, k, false)))
            }
        };
        self.freereg = saved;
        Ok(e)
    }

    fn table_ctor(&mut self, id: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        let Expr::Table { fields, .. } = self.ast.expr(id) else {
            unreachable!()
        };
        self.last_line = line;
        let treg = self.reserve(1)?;
        let (mut narr, mut nhash) = (0u32, 0u32);
        for f in fields {
            match f {
                TableField::Item(_) => narr += 1,
                _ => nhash += 1,
            }
        }
        self.emit(Inst::iabc(
            Op::NewTable,
            treg,
            narr.min(255),
            nhash.min(255),
            false,
        ));
        const FIELDS_PER_FLUSH: u32 = 50;
        let mut pending = 0u32; // items stacked above treg
        let mut flushed = 0u32; // number of items already in the table
        let fields: Vec<TableField> = fields.clone();
        for f in &fields {
            match f {
                TableField::Item(v) => {
                    // SETLIST requires items contiguous at treg+1..; nested
                    // expressions may move freereg, so pin the slot
                    let dst = treg + 1 + pending;
                    self.freereg = dst;
                    if self.freereg > MAX_REGS {
                        return Err(self.err(line, "constructor too long"));
                    }
                    if self.freereg > self.max_stack {
                        self.max_stack = self.freereg;
                    }
                    let ve = self.expr(*v)?;
                    self.freereg = dst;
                    let got = self.exp_to_nextreg(ve)?;
                    debug_assert_eq!(got, dst);
                    pending += 1;
                    if pending == FIELDS_PER_FLUSH {
                        self.setlist(treg, pending, flushed)?;
                        flushed += pending;
                        pending = 0;
                    }
                }
                TableField::Named(name, v) => {
                    let name = name.clone();
                    let saved = self.freereg;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    let c = self.str_const(name.text.as_bytes());
                    if c <= 0xFF {
                        self.emit(Inst::iabc(Op::SetField, treg, c, vr, true));
                    } else {
                        let kr = self.reserve(1)?;
                        self.load_const(kr, c);
                        self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    }
                    self.freereg = saved;
                }
                TableField::Keyed(k, v) => {
                    let saved = self.freereg;
                    let ke = self.expr(*k)?;
                    let kr = self.exp_to_anyreg(ke)?;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    self.freereg = saved;
                }
            }
        }
        if pending > 0 {
            self.setlist(treg, pending, flushed)?;
        }
        self.freereg = treg + 1;
        Ok(Exp::Reg(treg))
    }

    fn setlist(&mut self, treg: u32, n: u32, flushed: u32) -> Result<(), SyntaxError> {
        // batch index in C (×FIELDS_PER_FLUSH base handled by VM via flushed)
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, n, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, n, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    // ---- statements ----

    fn stat_block(&mut self, b: &Block) -> Result<(), SyntaxError> {
        for &sid in &b.stats {
            self.stat(sid)?;
            debug_assert!(
                self.freereg >= self.locals.last().map(|l| l.reg + 1).unwrap_or(0),
                "freereg sank below locals"
            );
        }
        Ok(())
    }

    fn block_scoped(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(b)?;
        self.leave_block();
        Ok(())
    }

    fn stat(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        match self.ast.stat(sid) {
            Stat::Do(b) => self.block_scoped(b),
            Stat::Local {
                collective,
                names,
                exprs,
            } => {
                let _ = collective; // attrib enforcement: slice 5
                let names: Vec<AttribName> = names.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                self.local_stat(&names, &exprs)
            }
            Stat::Assign { targets, exprs } => {
                let targets: Vec<ExprId> = targets.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                self.assign_stat(&targets, &exprs)
            }
            Stat::If { arms, else_body } => {
                let arms: Vec<(ExprId, Block)> = arms.clone();
                let else_body: Option<Block> = else_body.clone();
                self.if_stat(&arms, else_body.as_ref())
            }
            Stat::While { cond, body } => {
                let (cond, body) = (*cond, body.clone());
                self.while_stat(cond, &body)
            }
            Stat::Repeat { body, cond } => {
                let (body, cond) = (body.clone(), *cond);
                self.repeat_stat(&body, cond)
            }
            Stat::NumericFor {
                var,
                start,
                limit,
                step,
                body,
            } => {
                let var = var.clone();
                let (start, limit, step) = (*start, *limit, *step);
                let body = body.clone();
                self.numeric_for(&var.text, var.line, start, limit, step, &body)
            }
            Stat::Break { line } => {
                self.last_line = *line;
                let jmp = self.emit_jump();
                match self.blocks.iter_mut().rev().find(|b| b.is_loop) {
                    Some(b) => {
                        b.breaks.push(jmp);
                        Ok(())
                    }
                    None => Err(self.err(*line, "break outside a loop")),
                }
            }
            Stat::Return { exprs, line } => {
                let exprs: Vec<ExprId> = exprs.clone();
                self.last_line = *line;
                self.return_stat(&exprs)
            }
            Stat::Call(e) => {
                let line = match self.ast.expr(*e) {
                    Expr::Call { line, .. } | Expr::MethodCall { line, .. } => *line,
                    _ => self.last_line,
                };
                Err(self.err(line, "function calls are not supported yet (P03 slice 3)"))
            }
            Stat::GenericFor { .. } => Err(self.err(
                self.last_line,
                "generic 'for' is not supported yet (P03 slice 3)",
            )),
            Stat::Function { name, .. } => Err(self.err(
                name.base.line,
                "function statements are not supported yet (P03 slice 3)",
            )),
            Stat::LocalFunction { name, .. } | Stat::GlobalFunction { name, .. } => Err(self.err(
                name.line,
                "function statements are not supported yet (P03 slice 3)",
            )),
            Stat::Global { .. } | Stat::GlobalAll { .. } => Err(self.err(
                self.last_line,
                "global declarations are not supported yet (P03 slice 5)",
            )),
            Stat::Goto(n) | Stat::Label(n) => {
                Err(self.err(n.line, "goto/labels are not supported yet (P03 slice 5)"))
            }
        }
    }

    fn local_stat(&mut self, names: &[AttribName], exprs: &[ExprId]) -> Result<(), SyntaxError> {
        let base = self.freereg;
        let n = names.len() as u32;
        // evaluate initializers into consecutive fresh registers
        for (i, &e) in exprs.iter().enumerate() {
            let ee = self.expr(e)?;
            if (i as u32) < n {
                self.exp_to_nextreg(ee)?;
            } else {
                // extra initializer: evaluate for effects, discard
                let saved = self.freereg;
                self.exp_to_anyreg(ee)?;
                self.freereg = saved;
            }
        }
        let got = exprs.len() as u32;
        if got < n {
            let first = self.reserve(n - got)?;
            self.emit(Inst::iabc(Op::LoadNil, first, n - got - 1, 0, false));
        }
        for (i, an) in names.iter().enumerate() {
            self.declare_local(&an.name.text, base + i as u32, false);
        }
        self.freereg = base + n;
        Ok(())
    }

    fn assign_stat(&mut self, targets: &[ExprId], exprs: &[ExprId]) -> Result<(), SyntaxError> {
        let saved = self.freereg;
        // evaluate all values to fresh consecutive registers (right count)
        let n = targets.len();
        let mut vals = Vec::with_capacity(n);
        for (i, &e) in exprs.iter().enumerate() {
            let ee = self.expr(e)?;
            if i < n {
                vals.push(self.exp_to_nextreg(ee)?);
            } else {
                let s = self.freereg;
                self.exp_to_anyreg(ee)?;
                self.freereg = s;
            }
        }
        while vals.len() < n {
            let r = self.reserve(1)?;
            self.emit(Inst::iabc(Op::LoadNil, r, 0, 0, false));
            vals.push(r);
        }
        for (i, &t) in targets.iter().enumerate() {
            self.assign_to(t, vals[i])?;
        }
        self.freereg = saved;
        Ok(())
    }

    fn assign_to(&mut self, target: ExprId, vreg: u32) -> Result<(), SyntaxError> {
        match self.ast.expr(target) {
            Expr::Name(n) => {
                self.last_line = n.line;
                let text = n.text.clone();
                if let Some(l) = self.resolve_local(&text) {
                    if l.read_only {
                        return Err(self.err(
                            n.line,
                            format!("attempt to assign to const variable '{text}'"),
                        ));
                    }
                    let reg = l.reg;
                    self.emit(Inst::iabc(Op::Move, reg, vreg, 0, false));
                    return Ok(());
                }
                // global assignment: _ENV[name] = v
                let c = self.str_const(text.as_bytes());
                if c <= 0xFF {
                    self.emit(Inst::iabc(Op::SetTabUp, 0, c, vreg, true));
                } else {
                    let saved = self.freereg;
                    let r = self.reserve(2)?;
                    self.emit(Inst::iabc(Op::GetUpval, r, 0, 0, false));
                    self.load_const(r + 1, c);
                    self.emit(Inst::iabc(Op::SetTable, r, r + 1, vreg, false));
                    self.freereg = saved;
                }
                Ok(())
            }
            Expr::Index { obj, key } => {
                let (obj, key) = (*obj, *key);
                let saved = self.freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                match self.ast.expr(key) {
                    Expr::Str(s) if s.len() <= 255 => {
                        let s = s.clone();
                        let c = self.str_const(&s);
                        if c <= 0xFF {
                            self.emit(Inst::iabc(Op::SetField, o, c, vreg, true));
                        } else {
                            let kr = self.reserve(1)?;
                            self.load_const(kr, c);
                            self.emit(Inst::iabc(Op::SetTable, o, kr, vreg, false));
                        }
                    }
                    Expr::Int(i) if (0..=255).contains(i) => {
                        let c = *i as u32;
                        self.emit(Inst::iabc(Op::SetI, o, c, vreg, false));
                    }
                    _ => {
                        let ke = self.expr(key)?;
                        let k = self.exp_to_anyreg(ke)?;
                        self.emit(Inst::iabc(Op::SetTable, o, k, vreg, false));
                    }
                }
                self.freereg = saved;
                Ok(())
            }
            _ => unreachable!("parser validates assignment targets"),
        }
    }

    fn if_stat(
        &mut self,
        arms: &[(ExprId, Block)],
        else_body: Option<&Block>,
    ) -> Result<(), SyntaxError> {
        let mut end_jumps = Vec::new();
        for (i, (cond, body)) in arms.iter().enumerate() {
            let skip = self.cond_jump_false(*cond)?;
            self.block_scoped(body)?;
            let is_last = i == arms.len() - 1 && else_body.is_none();
            if !is_last {
                end_jumps.push(self.emit_jump());
            }
            self.patch_jump(skip)?;
        }
        if let Some(eb) = else_body {
            self.block_scoped(eb)?;
        }
        for j in end_jumps {
            self.patch_jump(j)?;
        }
        Ok(())
    }

    fn while_stat(&mut self, cond: ExprId, body: &Block) -> Result<(), SyntaxError> {
        let top = self.code.len();
        let exit = self.cond_jump_false(cond)?;
        self.enter_block(true);
        self.stat_block(body)?;
        self.jump_back(top)?;
        self.leave_loop_block()?;
        self.patch_jump(exit)?;
        Ok(())
    }

    fn repeat_stat(&mut self, body: &Block, cond: ExprId) -> Result<(), SyntaxError> {
        let top = self.code.len();
        // repeat body scope extends over the condition (Lua scoping rule)
        self.enter_block(true);
        self.stat_block(body)?;
        // until cond: loop again when cond is false
        let e = self.expr(cond)?;
        let saved = self.freereg;
        match e {
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.freereg = saved;
        self.jump_back(top)?;
        self.leave_loop_block()?;
        Ok(())
    }

    fn numeric_for(
        &mut self,
        var: &str,
        line: u32,
        start: ExprId,
        limit: ExprId,
        step: Option<ExprId>,
        body: &Block,
    ) -> Result<(), SyntaxError> {
        self.last_line = line;
        let base = self.reserve(3)?;
        let se = self.expr(start)?;
        self.exp_to_reg(se, base)?;
        let le = self.expr(limit)?;
        self.exp_to_reg(le, base + 1)?;
        match step {
            Some(st) => {
                let ste = self.expr(st)?;
                self.exp_to_reg(ste, base + 2)?;
            }
            None => {
                self.emit(Inst::iasbx(Op::LoadI, base + 2, 1));
            }
        }
        self.freereg = base + 3;
        let var_reg = self.reserve(1)?;
        self.enter_block(true);
        // 5.5: the control variable is read-only
        self.declare_local(var, var_reg, self.version >= LuaVersion::Lua55);
        let prep = self.emit(Inst::iabx(Op::ForPrep, base, 0));
        let body_top = self.code.len();
        self.stat_block(body)?;
        let loop_pc = self.code.len();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.emit(Inst::iabx(Op::ForLoop, base, back as u32));
        // FORPREP skips to just after FORLOOP when the loop runs zero times
        let skip = self.code.len() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.code[prep] = Inst::iabx(Op::ForPrep, base, skip as u32);
        self.leave_loop_block()?;
        self.freereg = base;
        Ok(())
    }

    fn return_stat(&mut self, exprs: &[ExprId]) -> Result<(), SyntaxError> {
        match exprs.len() {
            0 => {
                self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
            }
            1 => {
                let e = self.expr(exprs[0])?;
                let saved = self.freereg;
                let r = self.exp_to_anyreg(e)?;
                self.freereg = saved;
                self.emit(Inst::iabc(Op::Return1, r, 0, 0, false));
            }
            n => {
                let base = self.freereg;
                for &e in exprs {
                    let ee = self.expr(e)?;
                    self.exp_to_nextreg(ee)?;
                }
                self.freereg = base;
                self.emit(Inst::iabc(Op::Return, base, n as u32 + 1, 0, false));
            }
        }
        Ok(())
    }
}

/// Constant-fold arithmetic over two numeric literals where Lua semantics
/// are total (no division-by-zero style runtime errors).
fn fold_arith(op: BinOp, le: &Exp, ast: &Chunk, rhs: ExprId) -> Option<Exp> {
    let l = match le {
        Exp::Int(i) => Num::Int(*i),
        Exp::Float(f) => Num::Float(*f),
        _ => return None,
    };
    let r = match ast.expr(rhs) {
        Expr::Int(i) => Num::Int(*i),
        Expr::Float(f) => Num::Float(*f),
        _ => return None,
    };
    use Num::*;
    let v = match (op, l, r) {
        (BinOp::Add, Int(a), Int(b)) => Int(a.wrapping_add(b)),
        (BinOp::Sub, Int(a), Int(b)) => Int(a.wrapping_sub(b)),
        (BinOp::Mul, Int(a), Int(b)) => Int(a.wrapping_mul(b)),
        (BinOp::Add, a, b) => Float(a.as_f64() + b.as_f64()),
        (BinOp::Sub, a, b) => Float(a.as_f64() - b.as_f64()),
        (BinOp::Mul, a, b) => Float(a.as_f64() * b.as_f64()),
        (BinOp::Div, a, b) => Float(a.as_f64() / b.as_f64()),
        _ => return None,
    };
    Some(match v {
        Int(i) => Exp::Int(i),
        Float(f) => Exp::Float(f),
    })
}

use crate::numeric::Num;
