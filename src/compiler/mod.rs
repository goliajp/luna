//! AST → bytecode compiler. Register model follows PUC lparser/lcode:
//! locals pin the low registers, temporaries grow from `freereg`, constants
//! are deduplicated, forward jumps are patch lists (plain Vecs instead of
//! PUC's in-code jump chains). Function nesting is a stack of `Level`s;
//! upvalue resolution walks it (PUC singlevaraux).
//!
//! Slice 3 state: calls, closures, upvalues, varargs (5.5 table semantics),
//! generic `for`, multret, tail calls. Still pending (slice 5): goto/labels,
//! `<close>`, `global` declarations.

use std::collections::HashMap;

use crate::frontend::ast::{
    self, AttribName, BinOp, Block, Chunk, Expr, ExprId, FuncBody, Stat, StatId, TableField, UnOp,
};
use crate::frontend::error::SyntaxError;
use crate::numeric::Num;
use crate::runtime::heap::{GcHeader, ObjTag};
use crate::runtime::{Gc, Heap, LuaStr, Proto, UpvalDesc, Value};
use crate::version::LuaVersion;
use crate::vm::isa::{Inst, MAX_BX, MAX_SJ, Op};

pub fn compile_chunk(
    ast: &Chunk,
    version: LuaVersion,
    source_name: &[u8],
    heap: &mut Heap,
) -> Result<Gc<Proto>, SyntaxError> {
    let source = heap.intern(source_name);
    let mut c = Compiler {
        ast,
        heap,
        version,
        source,
        levels: Vec::new(),
        last_line: 0,
    };
    let mut main = Level::new(0, true, 0);
    main.upvals.push(UpvalDesc {
        in_stack: false,
        index: 0,
        name: "_ENV".into(),
    });
    c.levels.push(main);
    c.enter_block(false);
    c.stat_block(&ast.block)?;
    c.leave_block()?;
    c.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
    let lvl = c.levels.pop().expect("main level");
    Ok(c.heap.adopt_proto(lvl.into_proto(source, 0)))
}

const MAX_REGS: u32 = 254;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ConstKey {
    Int(i64),
    Float(u64),
    Str(*mut LuaStr),
}

struct LocalVar {
    name: Box<str>,
    reg: u32,
    read_only: bool,
    captured: bool,
}

struct BlockCx {
    first_local: usize,
    reg_floor: u32,
    is_loop: bool,
    breaks: Vec<usize>,
}

enum VarKind {
    Local(u32),
    Upval(u32),
    Global,
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
    /// comparison not yet materialized
    Cmp {
        op: Op,
        l: u32,
        r: u32,
    },
    /// open multi-result producer (CALL/VARARG) at `pc`, results from `base`
    Open {
        pc: usize,
        base: u32,
    },
}

struct Level {
    code: Vec<Inst>,
    lines: Vec<u32>,
    consts: Vec<Value>,
    const_map: HashMap<ConstKey, u32>,
    locals: Vec<LocalVar>,
    blocks: Vec<BlockCx>,
    freereg: u32,
    max_stack: u32,
    upvals: Vec<UpvalDesc>,
    protos: Vec<Gc<Proto>>,
    num_params: u8,
    is_vararg: bool,
    #[allow(dead_code)]
    line_defined: u32,
}

impl Level {
    fn new(num_params: u8, is_vararg: bool, line_defined: u32) -> Level {
        Level {
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
            const_map: HashMap::new(),
            locals: Vec::new(),
            blocks: Vec::new(),
            freereg: num_params as u32,
            max_stack: (num_params as u32).max(2),
            upvals: Vec::new(),
            protos: Vec::new(),
            num_params,
            is_vararg,
            line_defined,
        }
    }

    fn into_proto(self, source: Gc<LuaStr>, line_defined: u32) -> Proto {
        Proto {
            hdr: GcHeader::new(ObjTag::Proto),
            code: self.code.into_boxed_slice(),
            consts: self.consts.into_boxed_slice(),
            protos: self.protos.into_boxed_slice(),
            upvals: self.upvals.into_boxed_slice(),
            num_params: self.num_params,
            is_vararg: self.is_vararg,
            max_stack: self.max_stack as u8,
            lines: self.lines.into_boxed_slice(),
            source,
            line_defined,
        }
    }
}

struct Compiler<'a> {
    ast: &'a Chunk,
    heap: &'a mut Heap,
    version: LuaVersion,
    source: Gc<LuaStr>,
    levels: Vec<Level>,
    last_line: u32,
}

impl<'a> Compiler<'a> {
    // ---- infrastructure ----

    fn l(&mut self) -> &mut Level {
        self.levels.last_mut().expect("no level")
    }

    fn lr(&self) -> &Level {
        self.levels.last().expect("no level")
    }

    fn err(&self, line: u32, msg: impl Into<String>) -> SyntaxError {
        SyntaxError {
            line,
            msg: msg.into(),
        }
    }

    fn emit(&mut self, i: Inst) -> usize {
        let line = self.last_line;
        let l = self.l();
        l.code.push(i);
        l.lines.push(line);
        l.code.len() - 1
    }

    fn emit_jump(&mut self) -> usize {
        self.emit(Inst::isj(Op::Jmp, 0))
    }

    fn here(&self) -> usize {
        self.lr().code.len()
    }

    fn patch_jump(&mut self, pc: usize) -> Result<(), SyntaxError> {
        let target = self.here();
        let off = target as i64 - pc as i64 - 1;
        if off.unsigned_abs() > MAX_SJ as u64 {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.l().code[pc].set_sj(off as i32);
        Ok(())
    }

    fn jump_back(&mut self, target: usize) -> Result<(), SyntaxError> {
        let off = target as i64 - self.here() as i64 - 1;
        if off.unsigned_abs() > MAX_SJ as u64 {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.emit(Inst::isj(Op::Jmp, off as i32));
        Ok(())
    }

    fn reserve(&mut self, n: u32) -> Result<u32, SyntaxError> {
        let line = self.last_line;
        let l = self.l();
        let base = l.freereg;
        l.freereg += n;
        if l.freereg > MAX_REGS {
            return Err(self.err(line, "function or expression needs too many registers"));
        }
        if l.freereg > l.max_stack {
            l.max_stack = l.freereg;
        }
        Ok(base)
    }

    fn set_freereg(&mut self, r: u32) {
        let l = self.l();
        l.freereg = r;
        if r > l.max_stack {
            l.max_stack = r;
        }
    }

    fn const_idx(&mut self, key: ConstKey, v: Value) -> u32 {
        let l = self.l();
        if let Some(&i) = l.const_map.get(&key) {
            return i;
        }
        let i = l.consts.len() as u32;
        l.consts.push(v);
        l.const_map.insert(key, i);
        i
    }

    fn str_const(&mut self, bytes: &[u8]) -> u32 {
        let s = self.heap.intern(bytes);
        self.const_idx(ConstKey::Str(s.as_ptr()), Value::Str(s))
    }

    fn load_const(&mut self, reg: u32, c: u32) {
        if c <= MAX_BX {
            self.emit(Inst::iabx(Op::LoadK, reg, c));
        } else {
            self.emit(Inst::iabx(Op::LoadKx, reg, 0));
            self.emit(Inst::iax(Op::ExtraArg, c));
        }
    }

    /// Rewrite the wanted-results field (C) of an open CALL/VARARG.
    fn patch_wanted(&mut self, pc: usize, wanted_plus1: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst((i.0 & 0x00FF_FFFF) | (wanted_plus1 << 24));
    }

    /// Rewrite the destination (A) field of a pending instruction.
    fn patch_dest(&mut self, pc: usize, reg: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst(i.0 & !(0xFF << 7) | (reg << 7));
    }

    // ---- scopes & names ----

    fn enter_block(&mut self, is_loop: bool) {
        let floor = self.lr().freereg;
        let first = self.lr().locals.len();
        self.l().blocks.push(BlockCx {
            first_local: first,
            reg_floor: floor,
            is_loop,
            breaks: Vec::new(),
        });
    }

    fn leave_block(&mut self) -> Result<(), SyntaxError> {
        let b = self.l().blocks.pop().expect("block underflow");
        let captured = self.lr().locals[b.first_local..].iter().any(|l| l.captured);
        self.l().locals.truncate(b.first_local);
        self.set_freereg(b.reg_floor);
        if captured {
            self.emit(Inst::iabc(Op::Close, b.reg_floor, 0, 0, false));
        }
        for pc in b.breaks {
            self.patch_jump(pc)?;
        }
        Ok(())
    }

    fn block_captured(&self) -> bool {
        let b = self.lr().blocks.last().expect("no block");
        self.lr().locals[b.first_local..].iter().any(|l| l.captured)
    }

    fn block_floor(&self) -> u32 {
        self.lr().blocks.last().expect("no block").reg_floor
    }

    fn declare_local(&mut self, name: &str, reg: u32, read_only: bool) {
        self.l().locals.push(LocalVar {
            name: name.into(),
            reg,
            read_only,
            captured: false,
        });
    }

    fn resolve_name(&mut self, name: &str) -> VarKind {
        let top = self.levels.len() - 1;
        self.resolve_at(top, name)
    }

    fn resolve_at(&mut self, li: usize, name: &str) -> VarKind {
        if let Some(idx) = self.levels[li]
            .locals
            .iter()
            .rposition(|l| &*l.name == name)
        {
            return VarKind::Local(self.levels[li].locals[idx].reg);
        }
        if li < self.levels.len() - 1 || li == 0 {
            // upvalue cache applies at every level; main level has _ENV
            if let Some(ui) = self.levels[li].upvals.iter().position(|u| &*u.name == name) {
                return VarKind::Upval(ui as u32);
            }
        } else if let Some(ui) = self.levels[li].upvals.iter().position(|u| &*u.name == name) {
            return VarKind::Upval(ui as u32);
        }
        if li == 0 {
            return VarKind::Global;
        }
        match self.resolve_at(li - 1, name) {
            VarKind::Global => VarKind::Global,
            VarKind::Local(reg) => {
                if let Some(idx) = self.levels[li - 1]
                    .locals
                    .iter()
                    .rposition(|l| l.reg == reg && &*l.name == name)
                {
                    self.levels[li - 1].locals[idx].captured = true;
                }
                let ui = self.levels[li].upvals.len() as u32;
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: true,
                    index: reg as u8,
                    name: name.into(),
                });
                VarKind::Upval(ui)
            }
            VarKind::Upval(pidx) => {
                let ui = self.levels[li].upvals.len() as u32;
                self.levels[li].upvals.push(UpvalDesc {
                    in_stack: false,
                    index: pidx as u8,
                    name: name.into(),
                });
                VarKind::Upval(ui)
            }
        }
    }

    fn local_is_read_only(&self, reg: u32) -> Option<&str> {
        self.lr()
            .locals
            .iter()
            .rev()
            .find(|l| l.reg == reg)
            .filter(|l| l.read_only)
            .map(|l| &*l.name)
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
                let text = n.text.clone();
                self.name_expr(&text)
            }
            Expr::Paren(inner) => {
                // parentheses truncate multiple results to exactly one
                let e = self.expr(*inner)?;
                if let Exp::Open { .. } = e {
                    Ok(Exp::Reg(self.exp_to_anyreg(e)?))
                } else {
                    Ok(e)
                }
            }
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
            Expr::Vararg => self.vararg_expr(),
            Expr::Call { .. } | Expr::MethodCall { .. } => self.call_expr(id),
            Expr::Function(body) => {
                let body = body.clone();
                self.function_exp(&body, false)
            }
        }
    }

    fn name_expr(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        match self.resolve_name(name) {
            VarKind::Local(reg) => Ok(Exp::Reg(reg)),
            VarKind::Upval(u) => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetUpval,
                0,
                u,
                0,
                false,
            )))),
            VarKind::Global => self.global_access(name),
        }
    }

    /// `_ENV[name]` with `_ENV` resolved through the scope chain (it can be
    /// shadowed by a local or captured as an upvalue).
    fn global_access(&mut self, name: &str) -> Result<Exp, SyntaxError> {
        let c = self.str_const(name.as_bytes());
        match self.resolve_name("_ENV") {
            VarKind::Upval(u) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetTabUp,
                0,
                u,
                c,
                true,
            )))),
            VarKind::Local(r) if c <= 0xFF => Ok(Exp::Reloc(self.emit(Inst::iabc(
                Op::GetField,
                0,
                r,
                c,
                true,
            )))),
            env => {
                // rare: huge constant index — go through registers
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global => unreachable!("_ENV always resolves"),
                }
                self.load_const(er + 1, c);
                self.set_freereg(er);
                Ok(Exp::Reloc(self.emit(Inst::iabc(
                    Op::GetTable,
                    0,
                    er,
                    er + 1,
                    false,
                ))))
            }
        }
    }

    fn vararg_expr(&mut self) -> Result<Exp, SyntaxError> {
        if !self.lr().is_vararg {
            return Err(self.err(self.last_line, "cannot use '...' outside a vararg function"));
        }
        let base = self.reserve(1)?;
        let pc = self.emit(Inst::iabc(Op::Vararg, base, 0, 2, false));
        Ok(Exp::Open { pc, base })
    }

    fn call_expr(&mut self, id: ExprId) -> Result<Exp, SyntaxError> {
        match self.ast.expr(id) {
            Expr::Call { func, args, line } => {
                let (func, args, line) = (*func, args.clone(), *line);
                let base = self.lr().freereg;
                let fe = self.expr(func)?;
                self.set_freereg(base);
                let r = self.exp_to_nextreg(fe)?;
                debug_assert_eq!(r, base);
                let (nfixed, open) = self.args_onto_stack(&args, base + 1)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 1 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            Expr::MethodCall {
                obj,
                method,
                args,
                line,
            } => {
                let (obj, method, args, line) = (*obj, method.clone(), args.clone(), *line);
                let base = self.lr().freereg;
                let oe = self.expr(obj)?;
                let o = self.exp_to_anyreg(oe)?;
                self.set_freereg(base);
                self.reserve(2)?;
                let c = self.str_const(method.text.as_bytes());
                self.last_line = line;
                if c <= 0xFF {
                    self.emit(Inst::iabc(Op::SelfOp, base, o, c, true));
                } else {
                    self.emit(Inst::iabc(Op::Move, base + 1, o, 0, false));
                    let kr = self.reserve(1)?;
                    self.load_const(kr, c);
                    self.emit(Inst::iabc(Op::GetTable, base, base + 1, kr, false));
                    self.set_freereg(base + 2);
                }
                let (nfixed, open) = self.args_onto_stack(&args, base + 2)?;
                self.last_line = line;
                let b = if open { 0 } else { nfixed + 2 };
                let pc = self.emit(Inst::iabc(Op::Call, base, b, 2, false));
                self.set_freereg(base + 1);
                Ok(Exp::Open { pc, base })
            }
            _ => unreachable!(),
        }
    }

    /// Stack call arguments at consecutive registers from `argbase`.
    /// Returns (fixed_arg_count, last_is_open).
    fn args_onto_stack(
        &mut self,
        args: &[ExprId],
        argbase: u32,
    ) -> Result<(u32, bool), SyntaxError> {
        for (i, &a) in args.iter().enumerate() {
            let dst = argbase + i as u32;
            if dst >= MAX_REGS {
                return Err(self.err(self.last_line, "too many arguments"));
            }
            self.set_freereg(dst);
            let last = i == args.len() - 1;
            let e = self.expr(a)?;
            if last && let Exp::Open { pc, base } = e {
                debug_assert_eq!(base, dst);
                self.patch_wanted(pc, 0);
                return Ok((args.len() as u32 - 1, true));
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        Ok((args.len() as u32, false))
    }

    fn function_exp(&mut self, body: &FuncBody, is_method: bool) -> Result<Exp, SyntaxError> {
        let line = body.line;
        let nparams = body.params.len() + is_method as usize;
        if nparams > 200 {
            return Err(self.err(line, "too many parameters"));
        }
        let is_vararg = !matches!(body.vararg, ast::Vararg::None);
        self.levels.push(Level::new(nparams as u8, is_vararg, line));
        self.enter_block(false);
        if is_method {
            self.declare_local("self", 0, false);
        }
        for (i, p) in body.params.iter().enumerate() {
            self.declare_local(&p.text, (i + is_method as usize) as u32, false);
        }
        if let ast::Vararg::Named(n) = &body.vararg {
            let name = n.text.clone();
            let r = self.reserve(1)?;
            self.emit(Inst::iabc(Op::GetVarg, r, 0, 0, false));
            // 5.5: the named vararg table is a read-only local
            self.declare_local(&name, r, true);
        }
        self.stat_block(&body.block)?;
        self.leave_block()?;
        self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
        let lvl = self.levels.pop().expect("function level");
        let source = self.source;
        let proto = self.heap.adopt_proto(lvl.into_proto(source, line));
        let idx = self.lr().protos.len() as u32;
        if idx > MAX_BX {
            return Err(self.err(line, "too many nested functions"));
        }
        self.l().protos.push(proto);
        self.last_line = line;
        Ok(Exp::Reloc(self.emit(Inst::iabx(Op::Closure, 0, idx))))
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
                if (-65535..=65535).contains(&i) {
                    self.emit(Inst::iasbx(Op::LoadI, reg, i as i32));
                } else {
                    let c = self.const_idx(ConstKey::Int(i), Value::Int(i));
                    self.load_const(reg, c);
                }
            }
            Exp::Float(f) => {
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
            Exp::Reloc(pc) => self.patch_dest(pc, reg),
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, true));
                self.emit(Inst::isj(Op::Jmp, 1));
                self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
                self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
            }
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                if base != reg {
                    self.emit(Inst::iabc(Op::Move, reg, base, 0, false));
                }
            }
        }
        Ok(())
    }

    fn exp_to_nextreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let reg = self.reserve(1)?;
        self.exp_to_reg(e, reg)?;
        Ok(reg)
    }

    fn exp_to_anyreg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        match e {
            Exp::Reg(r) => Ok(r),
            Exp::Open { pc, base } => {
                self.patch_wanted(pc, 2);
                Ok(base)
            }
            e => self.exp_to_nextreg(e),
        }
    }

    /// Compile a condition; the returned JMP pc is taken when it is FALSE.
    fn cond_jump_false(&mut self, id: ExprId) -> Result<usize, SyntaxError> {
        let saved = self.lr().freereg;
        let e = self.expr(id)?;
        match e {
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.set_freereg(saved);
        Ok(self.emit_jump())
    }

    fn unop(&mut self, op: UnOp, operand: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        self.last_line = line;
        let e = self.expr(operand)?;
        let saved = self.lr().freereg;
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
        self.set_freereg(saved);
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
        let saved = self.lr().freereg;
        let le = self.expr(lhs)?;
        if let Some(folded) = fold_arith(op, &le, self.ast, rhs) {
            return Ok(folded);
        }
        let l = self.exp_to_anyreg(le)?;
        let re = self.expr(rhs)?;
        let r = self.exp_to_anyreg(re)?;
        self.set_freereg(saved);
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
            BinOp::Ne => return self.negate_cmp(Op::Eq, l, r),
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

    /// `a ~= b`: comparison materialized with inverted k.
    fn negate_cmp(&mut self, op: Op, l: u32, r: u32) -> Result<Exp, SyntaxError> {
        let reg = self.reserve(1)?;
        self.l().freereg -= 1;
        self.emit(Inst::iabc(op, l, r, 0, false));
        self.emit(Inst::isj(Op::Jmp, 1));
        self.emit(Inst::iabc(Op::LFalseSkip, reg, 0, 0, false));
        self.emit(Inst::iabc(Op::LoadTrue, reg, 0, 0, false));
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
        let base = self.lr().freereg;
        let le = self.expr(lhs)?;
        self.set_freereg(base);
        let reg = self.exp_to_nextreg(le)?;
        debug_assert_eq!(reg, base);
        let k = op == BinOp::Or;
        self.emit(Inst::iabc(Op::Test, reg, 0, 0, k));
        let jmp = self.emit_jump();
        self.set_freereg(reg);
        let re = self.expr(rhs)?;
        self.set_freereg(reg);
        let got = self.exp_to_nextreg(re)?;
        debug_assert_eq!(got, reg);
        self.patch_jump(jmp)?;
        Ok(Exp::Reg(reg))
    }

    fn concat(&mut self, lhs: ExprId, rhs: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        let base = self.lr().freereg;
        let le = self.expr(lhs)?;
        self.set_freereg(base);
        let l0 = self.exp_to_nextreg(le)?;
        debug_assert_eq!(l0, base);
        let re = self.expr(rhs)?;
        self.set_freereg(base + 1);
        let r0 = self.exp_to_nextreg(re)?;
        debug_assert_eq!(r0, base + 1);
        self.set_freereg(base);
        self.last_line = line;
        self.emit(Inst::iabc(Op::Concat, base, 2, 0, false));
        Ok(Exp::Reg(base))
    }

    fn index_expr(&mut self, obj: ExprId, key: ExprId) -> Result<Exp, SyntaxError> {
        let saved = self.lr().freereg;
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
        self.set_freereg(saved);
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
        let mut pending = 0u32;
        let mut flushed = 0u32;
        let fields: Vec<TableField> = fields.clone();
        let n_items = fields
            .iter()
            .filter(|f| matches!(f, TableField::Item(_)))
            .count();
        let mut item_idx = 0usize;
        for f in &fields {
            match f {
                TableField::Item(v) => {
                    item_idx += 1;
                    let dst = treg + 1 + pending;
                    if dst >= MAX_REGS {
                        return Err(self.err(line, "constructor too long"));
                    }
                    self.set_freereg(dst);
                    let e = self.expr(*v)?;
                    // last positional item: calls/varargs stay open
                    if item_idx == n_items
                        && let Exp::Open { pc, base } = e
                    {
                        debug_assert_eq!(base, dst);
                        self.patch_wanted(pc, 0);
                        self.setlist_open(treg, flushed)?;
                        pending = 0;
                        continue;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
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
                    let saved = self.lr().freereg;
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
                    self.set_freereg(saved);
                }
                TableField::Keyed(k, v) => {
                    let saved = self.lr().freereg;
                    let ke = self.expr(*k)?;
                    let kr = self.exp_to_anyreg(ke)?;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    self.set_freereg(saved);
                }
            }
        }
        if pending > 0 {
            self.setlist(treg, pending, flushed)?;
        }
        self.set_freereg(treg + 1);
        Ok(Exp::Reg(treg))
    }

    fn setlist(&mut self, treg: u32, n: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, n, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, n, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    /// SETLIST with B=0: take items up to the runtime top.
    fn setlist_open(&mut self, treg: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, 0, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, 0, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    // ---- statements ----

    fn stat_block(&mut self, b: &Block) -> Result<(), SyntaxError> {
        for &sid in &b.stats {
            self.stat(sid)?;
        }
        Ok(())
    }

    fn block_scoped(&mut self, b: &Block) -> Result<(), SyntaxError> {
        self.enter_block(false);
        self.stat_block(b)?;
        self.leave_block()
    }

    fn stat(&mut self, sid: StatId) -> Result<(), SyntaxError> {
        match self.ast.stat(sid) {
            Stat::Do(b) => {
                let b = b.clone();
                self.block_scoped(&b)
            }
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
            Stat::GenericFor { vars, exprs, body } => {
                let vars = vars.clone();
                let exprs: Vec<ExprId> = exprs.clone();
                let body = body.clone();
                self.generic_for(&vars, &exprs, &body)
            }
            Stat::Break { line } => {
                self.last_line = *line;
                let Some(loop_floor) = self
                    .lr()
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.is_loop)
                    .map(|b| b.reg_floor)
                else {
                    return Err(self.err(*line, "break outside a loop"));
                };
                self.emit(Inst::iabc(Op::Close, loop_floor, 0, 0, false));
                let jmp = self.emit_jump();
                self.l()
                    .blocks
                    .iter_mut()
                    .rev()
                    .find(|b| b.is_loop)
                    .expect("loop block")
                    .breaks
                    .push(jmp);
                Ok(())
            }
            Stat::Return { exprs, line } => {
                let exprs: Vec<ExprId> = exprs.clone();
                self.last_line = *line;
                self.return_stat(&exprs)
            }
            Stat::Call(e) => {
                let e = *e;
                if let Expr::Call { line, .. } | Expr::MethodCall { line, .. } = self.ast.expr(e) {
                    self.last_line = *line;
                }
                let base = self.lr().freereg;
                let ce = self.call_expr(e)?;
                let Exp::Open { pc, .. } = ce else {
                    unreachable!()
                };
                self.patch_wanted(pc, 1); // statement call: zero results
                self.set_freereg(base);
                Ok(())
            }
            Stat::Function { name, body } => {
                let (name, body) = (name.clone(), body.clone());
                self.function_stat(&name, &body)
            }
            Stat::LocalFunction { name, body } => {
                let (name, body) = (name.clone(), body.clone());
                self.last_line = name.line;
                let reg = self.reserve(1)?;
                // declared before the body: the function can call itself
                self.declare_local(&name.text, reg, false);
                let f = self.function_exp(&body, false)?;
                self.exp_to_reg(f, reg)?;
                self.set_freereg(reg + 1);
                Ok(())
            }
            Stat::GlobalFunction { name, body } => {
                // 5.5 global-declaration checking arrives in slice 5; compile
                // as a plain global assignment for now
                let (name, body) = (name.clone(), body.clone());
                self.last_line = name.line;
                let saved = self.lr().freereg;
                let f = self.function_exp(&body, false)?;
                let r = self.exp_to_anyreg(f)?;
                self.assign_global(&name.text, r)?;
                self.set_freereg(saved);
                Ok(())
            }
            Stat::Global { .. } | Stat::GlobalAll { .. } => Err(self.err(
                self.last_line,
                "global declarations are not supported yet (P03 slice 5)",
            )),
            Stat::Goto(n) | Stat::Label(n) => {
                Err(self.err(n.line, "goto/labels are not supported yet (P03 slice 5)"))
            }
        }
    }

    fn function_stat(&mut self, name: &ast::FuncName, body: &FuncBody) -> Result<(), SyntaxError> {
        self.last_line = name.base.line;
        let is_method = name.method.is_some();
        let saved = self.lr().freereg;
        let f = self.function_exp(body, is_method)?;
        let freg = self.exp_to_anyreg(f)?;
        if name.path.is_empty() && name.method.is_none() {
            let text = name.base.text.clone();
            self.assign_name(&text, name.base.line, freg)?;
            self.set_freereg(saved);
            return Ok(());
        }
        // function a.b.c:m — walk to the holder, set the final field
        let base_text = name.base.text.clone();
        let be = self.name_expr(&base_text)?;
        let mut holder = self.exp_to_anyreg(be)?;
        let mut fields: Vec<Box<str>> = name.path.iter().map(|n| n.text.clone()).collect();
        if let Some(m) = &name.method {
            fields.push(m.text.clone());
        }
        for f_name in &fields[..fields.len() - 1] {
            let c = self.str_const(f_name.as_bytes());
            if c <= 0xFF {
                let pc = self.emit(Inst::iabc(Op::GetField, 0, holder, c, true));
                let dst = self.reserve(1)?;
                self.patch_dest(pc, dst);
                holder = dst;
            } else {
                let kr = self.reserve(1)?;
                self.load_const(kr, c);
                let pc = self.emit(Inst::iabc(Op::GetTable, 0, holder, kr, false));
                self.patch_dest(pc, kr); // reuse the key register
                holder = kr;
            }
        }
        let last = &fields[fields.len() - 1];
        let c = self.str_const(last.as_bytes());
        if c <= 0xFF {
            self.emit(Inst::iabc(Op::SetField, holder, c, freg, true));
        } else {
            let kr = self.reserve(1)?;
            self.load_const(kr, c);
            self.emit(Inst::iabc(Op::SetTable, holder, kr, freg, false));
        }
        self.set_freereg(saved);
        Ok(())
    }

    fn local_stat(&mut self, names: &[AttribName], exprs: &[ExprId]) -> Result<(), SyntaxError> {
        let n = names.len() as u32;
        let base = self.explist_adjust(exprs, n)?;
        for (i, an) in names.iter().enumerate() {
            self.declare_local(&an.name.text, base + i as u32, false);
        }
        self.set_freereg(base + n);
        Ok(())
    }

    /// Evaluate an expression list into exactly `want` consecutive registers
    /// starting at the current freereg (nil-padded / truncated; an open last
    /// expression is patched to produce the balance). Returns the base.
    fn explist_adjust(&mut self, exprs: &[ExprId], want: u32) -> Result<u32, SyntaxError> {
        let base = self.lr().freereg;
        if exprs.is_empty() {
            if want > 0 {
                self.reserve(want)?;
                self.emit(Inst::iabc(Op::LoadNil, base, want - 1, 0, false));
            }
            return Ok(base);
        }
        let n = exprs.len() as u32;
        for (i, &eid) in exprs.iter().enumerate() {
            let dst = base + i as u32;
            if dst >= MAX_REGS {
                return Err(self.err(self.last_line, "too many values in expression list"));
            }
            self.set_freereg(dst);
            let e = self.expr(eid)?;
            let last = i as u32 == n - 1;
            if last && let Exp::Open { pc, base: ob } = e {
                debug_assert_eq!(ob, dst);
                let missing = (want + 1).saturating_sub(n); // results the open expr must provide
                self.patch_wanted(pc, missing + 1);
                self.set_freereg(base + want.max(n - 1));
                return Ok(base);
            }
            self.set_freereg(dst);
            let got = self.exp_to_nextreg(e)?;
            debug_assert_eq!(got, dst);
        }
        if n < want {
            let first = self.reserve(want - n)?;
            self.emit(Inst::iabc(Op::LoadNil, first, want - n - 1, 0, false));
        }
        self.set_freereg(base + want);
        Ok(base)
    }

    fn assign_stat(&mut self, targets: &[ExprId], exprs: &[ExprId]) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        let want = targets.len() as u32;
        let base = self.explist_adjust(exprs, want)?;
        for (i, &t) in targets.iter().enumerate() {
            self.assign_to(t, base + i as u32)?;
        }
        self.set_freereg(saved);
        Ok(())
    }

    fn assign_name(&mut self, text: &str, line: u32, vreg: u32) -> Result<(), SyntaxError> {
        self.last_line = line;
        match self.resolve_name(text) {
            VarKind::Local(reg) => {
                if let Some(name) = self.local_is_read_only(reg) {
                    let name = name.to_string();
                    return Err(self.err(
                        line,
                        format!("attempt to assign to const variable '{name}'"),
                    ));
                }
                if reg != vreg {
                    self.emit(Inst::iabc(Op::Move, reg, vreg, 0, false));
                }
                Ok(())
            }
            VarKind::Upval(u) => {
                self.emit(Inst::iabc(Op::SetUpval, vreg, u, 0, false));
                Ok(())
            }
            VarKind::Global => self.assign_global(text, vreg),
        }
    }

    fn assign_global(&mut self, text: &str, vreg: u32) -> Result<(), SyntaxError> {
        let c = self.str_const(text.as_bytes());
        match self.resolve_name("_ENV") {
            VarKind::Upval(u) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetTabUp, u, c, vreg, true));
                Ok(())
            }
            VarKind::Local(r) if c <= 0xFF => {
                self.emit(Inst::iabc(Op::SetField, r, c, vreg, true));
                Ok(())
            }
            env => {
                let saved = self.lr().freereg;
                let er = self.reserve(2)?;
                match env {
                    VarKind::Upval(u) => {
                        self.emit(Inst::iabc(Op::GetUpval, er, u, 0, false));
                    }
                    VarKind::Local(r) => {
                        self.emit(Inst::iabc(Op::Move, er, r, 0, false));
                    }
                    VarKind::Global => unreachable!("_ENV always resolves"),
                }
                self.load_const(er + 1, c);
                self.emit(Inst::iabc(Op::SetTable, er, er + 1, vreg, false));
                self.set_freereg(saved);
                Ok(())
            }
        }
    }

    fn assign_to(&mut self, target: ExprId, vreg: u32) -> Result<(), SyntaxError> {
        match self.ast.expr(target) {
            Expr::Name(n) => {
                let (text, line) = (n.text.clone(), n.line);
                self.assign_name(&text, line, vreg)
            }
            Expr::Index { obj, key } => {
                let (obj, key) = (*obj, *key);
                let saved = self.lr().freereg;
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
                self.set_freereg(saved);
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
        let top = self.here();
        let exit = self.cond_jump_false(cond)?;
        self.enter_block(true);
        self.stat_block(body)?;
        if self.block_captured() {
            let floor = self.block_floor();
            self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
        }
        self.jump_back(top)?;
        self.leave_block()?;
        self.patch_jump(exit)?;
        Ok(())
    }

    fn repeat_stat(&mut self, body: &Block, cond: ExprId) -> Result<(), SyntaxError> {
        let top = self.here();
        self.enter_block(true);
        self.stat_block(body)?;
        let e = self.expr(cond)?;
        let saved = self.lr().freereg;
        match e {
            Exp::Cmp { op, l, r } => {
                self.emit(Inst::iabc(op, l, r, 0, false));
            }
            e => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::Test, r, 0, 0, false));
            }
        }
        self.set_freereg(saved);
        if self.block_captured() {
            let floor = self.block_floor();
            self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
        }
        self.jump_back(top)?;
        self.leave_block()?;
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
        let base = self.lr().freereg;
        self.set_freereg(base);
        let se = self.expr(start)?;
        self.set_freereg(base);
        let s0 = self.exp_to_nextreg(se)?;
        debug_assert_eq!(s0, base);
        let le = self.expr(limit)?;
        self.set_freereg(base + 1);
        let l0 = self.exp_to_nextreg(le)?;
        debug_assert_eq!(l0, base + 1);
        match step {
            Some(st) => {
                let ste = self.expr(st)?;
                self.set_freereg(base + 2);
                let st0 = self.exp_to_nextreg(ste)?;
                debug_assert_eq!(st0, base + 2);
            }
            None => {
                self.set_freereg(base + 2);
                self.reserve(1)?;
                self.emit(Inst::iasbx(Op::LoadI, base + 2, 1));
            }
        }
        self.set_freereg(base + 3);
        self.enter_block(true);
        let var_reg = self.reserve(1)?;
        self.declare_local(var, var_reg, self.version >= LuaVersion::Lua55);
        let prep = self.emit(Inst::iabx(Op::ForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.emit(Inst::iabc(Op::Close, var_reg, 0, 0, false));
        }
        let loop_pc = self.here();
        let back = loop_pc - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.emit(Inst::iabx(Op::ForLoop, base, back as u32));
        let skip = self.here() - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::ForPrep, base, skip as u32);
        self.leave_block()?;
        self.set_freereg(base);
        Ok(())
    }

    fn generic_for(
        &mut self,
        vars: &[ast::Name],
        exprs: &[ExprId],
        body: &Block,
    ) -> Result<(), SyntaxError> {
        let line = vars[0].line;
        self.last_line = line;
        // control slots: iterator, state, control, closing (<close>: slice 5)
        let base = self.explist_adjust(exprs, 4)?;
        self.set_freereg(base + 4);
        self.enter_block(true);
        let nvars = vars.len() as u32;
        let vbase = self.reserve(nvars)?;
        debug_assert_eq!(vbase, base + 4);
        for (i, v) in vars.iter().enumerate() {
            // 5.5: the control (first) variable is read-only
            self.declare_local(
                &v.text,
                vbase + i as u32,
                i == 0 && self.version >= LuaVersion::Lua55,
            );
        }
        let prep = self.emit(Inst::iabx(Op::TForPrep, base, 0));
        let body_top = self.here();
        self.stat_block(body)?;
        if self.block_captured() {
            self.emit(Inst::iabc(Op::Close, vbase, 0, 0, false));
        }
        let tforcall_pc = self.here();
        let skip = tforcall_pc - prep - 1;
        if skip as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.l().code[prep] = Inst::iabx(Op::TForPrep, base, skip as u32);
        self.emit(Inst::iabc(Op::TForCall, base, 0, nvars, false));
        let back = self.here() - body_top + 1;
        if back as u32 > MAX_BX {
            return Err(self.err(line, "control structure too long"));
        }
        self.emit(Inst::iabx(Op::TForLoop, base, back as u32));
        self.leave_block()?;
        self.set_freereg(base);
        Ok(())
    }

    fn return_stat(&mut self, exprs: &[ExprId]) -> Result<(), SyntaxError> {
        match exprs.len() {
            0 => {
                self.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
            }
            1 => {
                // tail call: `return f(...)` (not parenthesized)
                if matches!(
                    self.ast.expr(exprs[0]),
                    Expr::Call { .. } | Expr::MethodCall { .. }
                ) {
                    let base = self.lr().freereg;
                    let e = self.call_expr(exprs[0])?;
                    let Exp::Open { pc, base: cb } = e else {
                        unreachable!()
                    };
                    debug_assert_eq!(cb, base);
                    let call = self.l().code[pc];
                    self.l().code[pc] = Inst::iabc(Op::TailCall, call.a(), call.b(), 0, false);
                    self.set_freereg(base);
                    return Ok(());
                }
                if matches!(self.ast.expr(exprs[0]), Expr::Vararg) {
                    let base = self.lr().freereg;
                    let e = self.expr(exprs[0])?;
                    let Exp::Open { pc, .. } = e else {
                        unreachable!()
                    };
                    self.patch_wanted(pc, 0);
                    self.emit(Inst::iabc(Op::Return, base, 0, 0, false));
                    self.set_freereg(base);
                    return Ok(());
                }
                let e = self.expr(exprs[0])?;
                let saved = self.lr().freereg;
                let r = self.exp_to_anyreg(e)?;
                self.set_freereg(saved);
                self.emit(Inst::iabc(Op::Return1, r, 0, 0, false));
            }
            n => {
                let base = self.lr().freereg;
                let mut open = false;
                for (i, &eid) in exprs.iter().enumerate() {
                    let dst = base + i as u32;
                    self.set_freereg(dst);
                    let e = self.expr(eid)?;
                    if i == n - 1
                        && let Exp::Open { pc, base: ob } = e
                    {
                        debug_assert_eq!(ob, dst);
                        self.patch_wanted(pc, 0);
                        open = true;
                        break;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
                    debug_assert_eq!(got, dst);
                }
                let b = if open { 0 } else { n as u32 + 1 };
                self.emit(Inst::iabc(Op::Return, base, b, 0, false));
                self.set_freereg(base);
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
