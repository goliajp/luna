//! The interpreter. Dispatch is a plain match over opcodes (the P10 ceiling
//! pass owns dispatch optimization). Lua→Lua calls do not recurse the Rust
//! stack; only native↔Lua boundaries do (slice 3).

use crate::compiler::compile_chunk;
use crate::frontend::{SyntaxError, parse};
use crate::numeric::{self, Num};
use crate::runtime::{Gc, Heap, LuaClosure, Table, TableError, UpvalState, Value};
use crate::version::LuaVersion;
use crate::vm::error::LuaError;
use crate::vm::isa::{Inst, Op};

pub struct Vm {
    pub heap: Heap,
    stack: Vec<Value>,
    frames: Vec<Frame>,
    globals: Gc<Table>,
    version: LuaVersion,
}

struct Frame {
    closure: Gc<LuaClosure>,
    /// stack index of register 0
    base: u32,
    pc: u32,
    /// stack slot of the function itself (results land here)
    func_slot: u32,
}

#[derive(Debug)]
pub enum Error {
    Syntax(SyntaxError),
    Runtime(LuaError),
}

impl From<SyntaxError> for Error {
    fn from(e: SyntaxError) -> Error {
        Error::Syntax(e)
    }
}

impl From<LuaError> for Error {
    fn from(e: LuaError) -> Error {
        Error::Runtime(e)
    }
}

impl Vm {
    pub fn new(version: LuaVersion) -> Vm {
        let mut heap = Heap::new();
        let globals = heap.new_table();
        Vm {
            heap,
            stack: Vec::new(),
            frames: Vec::new(),
            globals,
            version,
        }
    }

    pub fn globals(&self) -> Gc<Table> {
        self.globals
    }

    /// Parse + compile a chunk and close it over the globals table.
    pub fn load(&mut self, src: &[u8], chunkname: &[u8]) -> Result<Gc<LuaClosure>, SyntaxError> {
        let ast = parse(src, self.version)?;
        let proto = compile_chunk(&ast, self.version, chunkname, &mut self.heap)?;
        let env = self
            .heap
            .new_upvalue(UpvalState::Closed(Value::Table(self.globals)));
        Ok(self.heap.new_closure(proto, Box::new([env])))
    }

    /// Convenience for tests: load + run, returning the chunk's results.
    pub fn eval(&mut self, src: &str) -> Result<Vec<Value>, Error> {
        let cl = self.load(src.as_bytes(), b"=eval")?;
        Ok(self.call(cl, &[])?)
    }

    /// Render an error value for messages/tests.
    pub fn error_text(&self, e: &LuaError) -> String {
        match e.0 {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            v => format!("(error object is a {} value)", v.type_name()),
        }
    }

    pub fn call(&mut self, cl: Gc<LuaClosure>, args: &[Value]) -> Result<Vec<Value>, LuaError> {
        let func_slot = self.stack.len() as u32;
        self.stack.push(Value::Closure(cl));
        for &a in args {
            self.stack.push(a);
        }
        self.push_frame(cl, func_slot, args.len() as u32);
        let r = self.exec();
        if r.is_err() {
            // unwind everything this call created
            self.frames.truncate(0);
            self.stack.truncate(func_slot as usize);
        }
        r
    }

    fn push_frame(&mut self, cl: Gc<LuaClosure>, func_slot: u32, nargs: u32) {
        let proto = cl.proto;
        let base = func_slot + 1;
        let nparams = proto.num_params as u32;
        // missing fixed params become nil; extra args ignored (varargs: slice 3)
        let _ = nargs;
        let need = base + proto.max_stack as u32;
        let need = need.max(base + nparams);
        self.stack.resize(need as usize, Value::Nil);
        self.frames.push(Frame {
            closure: cl,
            base,
            pc: 0,
            func_slot,
        });
    }

    // ---- register / constant access ----

    #[inline(always)]
    fn r(&self, base: u32, i: u32) -> Value {
        self.stack[(base + i) as usize]
    }

    #[inline(always)]
    fn set_r(&mut self, base: u32, i: u32, v: Value) {
        self.stack[(base + i) as usize] = v;
    }

    fn upval(&self, cl: Gc<LuaClosure>, idx: u32) -> Value {
        match cl.upvals[idx as usize].state() {
            UpvalState::Open(slot) => self.stack[slot as usize],
            UpvalState::Closed(v) => v,
        }
    }

    // ---- error helpers ----

    fn rt_err(&mut self, msg: &str) -> LuaError {
        let f = self.frames.last().expect("error outside a frame");
        let proto = f.closure.proto;
        let line = proto.lines[(f.pc as usize).saturating_sub(1)];
        let src = String::from_utf8_lossy(numeric_src_name(proto.source.as_ptr())).into_owned();
        let text = format!("{src}:{line}: {msg}");
        LuaError(Value::Str(self.heap.intern(text.as_bytes())))
    }

    fn type_err(&mut self, what: &str, v: Value) -> LuaError {
        self.rt_err(&format!("attempt to {what} a {} value", v.type_name()))
    }

    // ---- the interpreter ----

    fn exec(&mut self) -> Result<Vec<Value>, LuaError> {
        let entry_depth = self.frames.len();
        loop {
            let f = self.frames.last().expect("no frame");
            let cl = f.closure;
            let base = f.base;
            let pc = f.pc;
            let inst = cl.proto.code[pc as usize];
            self.frames.last_mut().expect("no frame").pc = pc + 1;

            match inst.op() {
                Op::Move => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadI => self.set_r(base, inst.a(), Value::Int(inst.sbx() as i64)),
                Op::LoadF => self.set_r(base, inst.a(), Value::Float(inst.sbx() as f64)),
                Op::LoadK => {
                    let v = cl.proto.consts[inst.bx() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadKx => {
                    let extra = cl.proto.code[self.pc_of_top() as usize];
                    self.bump_pc();
                    let v = cl.proto.consts[extra.ax() as usize];
                    self.set_r(base, inst.a(), v);
                }
                Op::LoadFalse => self.set_r(base, inst.a(), Value::Bool(false)),
                Op::LFalseSkip => {
                    self.set_r(base, inst.a(), Value::Bool(false));
                    self.bump_pc();
                }
                Op::LoadTrue => self.set_r(base, inst.a(), Value::Bool(true)),
                Op::LoadNil => {
                    let a = inst.a();
                    for i in 0..=inst.b() {
                        self.set_r(base, a + i, Value::Nil);
                    }
                }
                Op::GetUpval => {
                    let v = self.upval(cl, inst.b());
                    self.set_r(base, inst.a(), v);
                }
                Op::SetUpval => {
                    return Err(self.rt_err("SETUPVAL before closures (slice 3)"));
                }
                Op::GetTabUp => {
                    let t = self.upval(cl, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetTable => {
                    let t = self.r(base, inst.b());
                    let key = self.r(base, inst.c());
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetI => {
                    let t = self.r(base, inst.b());
                    let v = self.index_value(t, Value::Int(inst.c() as i64))?;
                    self.set_r(base, inst.a(), v);
                }
                Op::GetField => {
                    let t = self.r(base, inst.b());
                    let key = cl.proto.consts[inst.c() as usize];
                    let v = self.index_value(t, key)?;
                    self.set_r(base, inst.a(), v);
                }
                Op::SetTabUp => {
                    let t = self.upval(cl, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::SetTable => {
                    let t = self.r(base, inst.a());
                    let key = self.r(base, inst.b());
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::SetI => {
                    let t = self.r(base, inst.a());
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, Value::Int(inst.b() as i64), v)?;
                }
                Op::SetField => {
                    let t = self.r(base, inst.a());
                    let key = cl.proto.consts[inst.b() as usize];
                    let v = self.r(base, inst.c());
                    self.newindex_value(t, key, v)?;
                }
                Op::NewTable => {
                    let t = self.heap.new_table();
                    self.set_r(base, inst.a(), Value::Table(t));
                }
                Op::SetList => {
                    let a = inst.a();
                    let n = inst.b();
                    let offset = if inst.k() {
                        let extra = cl.proto.code[self.pc_of_top() as usize];
                        self.bump_pc();
                        extra.ax() as i64
                    } else {
                        inst.c() as i64
                    };
                    let Value::Table(t) = self.r(base, a) else {
                        unreachable!("SETLIST on non-table");
                    };
                    for i in 1..=n {
                        let v = self.r(base, a + i);
                        unsafe { t.as_mut() }.set_int(offset + i as i64, v);
                    }
                }
                Op::SelfOp => {
                    return Err(self.rt_err("SELF before calls (slice 3)"));
                }
                Op::Add => self.arith_rr(inst, base, ArithOp::Add)?,
                Op::Sub => self.arith_rr(inst, base, ArithOp::Sub)?,
                Op::Mul => self.arith_rr(inst, base, ArithOp::Mul)?,
                Op::Mod => self.arith_rr(inst, base, ArithOp::Mod)?,
                Op::Pow => self.arith_rr(inst, base, ArithOp::Pow)?,
                Op::Div => self.arith_rr(inst, base, ArithOp::Div)?,
                Op::IDiv => self.arith_rr(inst, base, ArithOp::IDiv)?,
                Op::BAnd => self.arith_rr(inst, base, ArithOp::BAnd)?,
                Op::BOr => self.arith_rr(inst, base, ArithOp::BOr)?,
                Op::BXor => self.arith_rr(inst, base, ArithOp::BXor)?,
                Op::Shl => self.arith_rr(inst, base, ArithOp::Shl)?,
                Op::Shr => self.arith_rr(inst, base, ArithOp::Shr)?,
                Op::Unm => {
                    let v = self.r(base, inst.b());
                    let r = match v {
                        Value::Int(i) => Value::Int(i.wrapping_neg()),
                        Value::Float(f) => Value::Float(-f),
                        v => return Err(self.type_err("perform arithmetic on", v)),
                    };
                    self.set_r(base, inst.a(), r);
                }
                Op::BNot => {
                    let v = self.r(base, inst.b());
                    let i = self.int_from(v, "perform bitwise operation on")?;
                    self.set_r(base, inst.a(), Value::Int(!i));
                }
                Op::Not => {
                    let v = self.r(base, inst.b());
                    self.set_r(base, inst.a(), Value::Bool(!v.truthy()));
                }
                Op::Len => {
                    let v = self.r(base, inst.b());
                    let r = match v {
                        Value::Str(s) => Value::Int(s.len() as i64),
                        Value::Table(t) => Value::Int(t.len()),
                        v => return Err(self.type_err("get length of", v)),
                    };
                    self.set_r(base, inst.a(), r);
                }
                Op::Concat => {
                    let a = inst.a();
                    let n = inst.b();
                    let mut out: Vec<u8> = Vec::new();
                    for i in 0..n {
                        let v = self.r(base, a + i);
                        match v {
                            Value::Str(s) => out.extend_from_slice(s.as_bytes()),
                            Value::Int(x) => out
                                .extend_from_slice(numeric::num_to_string(Num::Int(x)).as_bytes()),
                            Value::Float(x) => out.extend_from_slice(
                                numeric::num_to_string(Num::Float(x)).as_bytes(),
                            ),
                            v => return Err(self.type_err("concatenate", v)),
                        }
                    }
                    let s = self.heap.intern(&out);
                    self.set_r(base, a, Value::Str(s));
                }
                Op::Close | Op::Tbc => {
                    // upvalue closing / to-be-closed: slices 3 and 5
                }
                Op::Jmp => {
                    self.add_pc(inst.sj());
                }
                Op::Eq => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = l.raw_eq(r);
                    self.cond_skip(cond, inst.k());
                }
                Op::EqK => {
                    let l = self.r(base, inst.a());
                    let r = cl.proto.consts[inst.b() as usize];
                    let cond = l.raw_eq(r);
                    self.cond_skip(cond, inst.k());
                }
                Op::Lt => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = self.less_than(l, r, false)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::Le => {
                    let l = self.r(base, inst.a());
                    let r = self.r(base, inst.b());
                    let cond = self.less_than(l, r, true)?;
                    self.cond_skip(cond, inst.k());
                }
                Op::Test => {
                    let cond = self.r(base, inst.a()).truthy();
                    self.cond_skip(cond, inst.k());
                }
                Op::TestSet => {
                    let v = self.r(base, inst.b());
                    if v.truthy() == inst.k() {
                        self.set_r(base, inst.a(), v);
                    } else {
                        self.bump_pc();
                    }
                }
                Op::Return | Op::Return0 | Op::Return1 => {
                    let results: Vec<Value> = match inst.op() {
                        Op::Return0 => Vec::new(),
                        Op::Return1 => vec![self.r(base, inst.a())],
                        _ => {
                            let a = inst.a();
                            let n = inst.b() - 1; // B >= 1 in slice 2
                            (0..n).map(|i| self.r(base, a + i)).collect()
                        }
                    };
                    let f = self.frames.pop().expect("no frame");
                    self.stack.truncate(f.func_slot as usize);
                    if self.frames.len() < entry_depth {
                        return Ok(results);
                    }
                    // nested Lua frames: slice 3 places results for the caller
                    return Err(self.rt_err("nested returns before calls (slice 3)"));
                }
                Op::ForPrep => self.for_prep(inst, base)?,
                Op::ForLoop => self.for_loop(inst, base),
                Op::Call
                | Op::TailCall
                | Op::TForPrep
                | Op::TForCall
                | Op::TForLoop
                | Op::Closure
                | Op::Vararg
                | Op::VarargPrep => {
                    return Err(self.rt_err("calls/closures are slice 3"));
                }
                Op::ExtraArg => unreachable!("EXTRAARG executed directly"),
            }
        }
    }

    #[inline(always)]
    fn pc_of_top(&self) -> u32 {
        self.frames.last().expect("no frame").pc
    }

    #[inline(always)]
    fn bump_pc(&mut self) {
        self.frames.last_mut().expect("no frame").pc += 1;
    }

    #[inline(always)]
    fn add_pc(&mut self, d: i32) {
        let f = self.frames.last_mut().expect("no frame");
        f.pc = (f.pc as i64 + d as i64) as u32;
    }

    /// PUC conditional-skip convention: the JMP that follows is executed when
    /// `cond == k`; otherwise it is skipped.
    #[inline(always)]
    fn cond_skip(&mut self, cond: bool, k: bool) {
        if cond != k {
            self.bump_pc();
        }
    }

    // ---- indexing (raw in slice 2; metamethods arrive in slice 4) ----

    fn index_value(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        match t {
            Value::Table(t) => Ok(t.get(key)),
            v => Err(self.type_err("index", v)),
        }
    }

    fn newindex_value(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        match t {
            Value::Table(t) => match unsafe { t.as_mut() }.set(key, v) {
                Ok(()) => Ok(()),
                Err(TableError::NilIndex) => Err(self.rt_err("table index is nil")),
                Err(TableError::NanIndex) => Err(self.rt_err("table index is NaN")),
                Err(TableError::InvalidNext) => unreachable!(),
            },
            v => Err(self.type_err("index", v)),
        }
    }

    // ---- arithmetic ----

    fn arith_rr(&mut self, inst: Inst, base: u32, op: ArithOp) -> Result<(), LuaError> {
        let l = self.r(base, inst.b());
        let r = self.r(base, inst.c());
        let v = self.arith(op, l, r)?;
        self.set_r(base, inst.a(), v);
        Ok(())
    }

    fn arith(&mut self, op: ArithOp, l: Value, r: Value) -> Result<Value, LuaError> {
        use ArithOp::*;
        match op {
            BAnd | BOr | BXor | Shl | Shr => {
                let a = self.int_from(l, "perform bitwise operation on")?;
                let b = self.int_from(r, "perform bitwise operation on")?;
                let v = match op {
                    BAnd => a & b,
                    BOr => a | b,
                    BXor => a ^ b,
                    Shl => shift_left(a, b),
                    Shr => shift_left(a, b.wrapping_neg()),
                    _ => unreachable!(),
                };
                return Ok(Value::Int(v));
            }
            _ => {}
        }
        let (ln, rn) = match (as_num(l), as_num(r)) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                let bad = if as_num(l).is_none() { l } else { r };
                return Err(self.type_err("perform arithmetic on", bad));
            }
        };
        match (op, ln, rn) {
            (Add, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_add(b))),
            (Sub, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_sub(b))),
            (Mul, Num::Int(a), Num::Int(b)) => Ok(Value::Int(a.wrapping_mul(b))),
            (IDiv, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to perform 'n//0'"));
                }
                let mut q = a.wrapping_div(b);
                if (a ^ b) < 0 && q.wrapping_mul(b) != a {
                    q -= 1;
                }
                Ok(Value::Int(q))
            }
            (Mod, Num::Int(a), Num::Int(b)) => {
                if b == 0 {
                    return Err(self.rt_err("attempt to perform 'n%0'"));
                }
                let mut m = a.wrapping_rem(b);
                if m != 0 && (m ^ b) < 0 {
                    m += b;
                }
                Ok(Value::Int(m))
            }
            (Add, a, b) => Ok(Value::Float(a.as_f64() + b.as_f64())),
            (Sub, a, b) => Ok(Value::Float(a.as_f64() - b.as_f64())),
            (Mul, a, b) => Ok(Value::Float(a.as_f64() * b.as_f64())),
            (Div, a, b) => Ok(Value::Float(a.as_f64() / b.as_f64())),
            (Pow, a, b) => Ok(Value::Float(a.as_f64().powf(b.as_f64()))),
            (IDiv, a, b) => Ok(Value::Float((a.as_f64() / b.as_f64()).floor())),
            (Mod, a, b) => {
                let (x, y) = (a.as_f64(), b.as_f64());
                let mut m = x % y;
                if m * y < 0.0 {
                    m += y;
                }
                Ok(Value::Float(m))
            }
            _ => unreachable!(),
        }
    }

    fn int_from(&mut self, v: Value, what: &str) -> Result<i64, LuaError> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Float(f) => match crate::runtime::value::f2i_exact(f) {
                Some(i) => Ok(i),
                None => Err(self.rt_err("number has no integer representation")),
            },
            v => Err(self.type_err(what, v)),
        }
    }

    // ---- comparison ----

    fn less_than(&mut self, l: Value, r: Value, or_eq: bool) -> Result<bool, LuaError> {
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => Ok(if or_eq { a <= b } else { a < b }),
            (Value::Float(a), Value::Float(b)) => Ok(if or_eq { a <= b } else { a < b }),
            (Value::Int(a), Value::Float(b)) => Ok(if or_eq {
                int_le_float(a, b)
            } else {
                int_lt_float(a, b)
            }),
            (Value::Float(a), Value::Int(b)) => Ok(if or_eq {
                // a <= b  ⟺  not (b < a)
                !int_lt_float(b, a)
            } else {
                !int_le_float(b, a)
            }),
            (Value::Str(a), Value::Str(b)) => {
                let (a, b) = (a.as_bytes(), b.as_bytes());
                Ok(if or_eq { a <= b } else { a < b })
            }
            (l, r) => Err(self.rt_err(&format!(
                "attempt to compare {} with {}",
                l.type_name(),
                r.type_name()
            ))),
        }
    }

    // ---- numeric for ----

    fn for_prep(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        let init = self.r(base, a);
        let limit = self.r(base, a + 1);
        let step = self.r(base, a + 2);
        let (Some(init_n), Some(limit_n), Some(step_n)) =
            (as_num(init), as_num(limit), as_num(step))
        else {
            let which = if as_num(init).is_none() {
                "initial"
            } else if as_num(limit).is_none() {
                "limit"
            } else {
                "step"
            };
            return Err(self.rt_err(&format!("'for' {which} value must be a number")));
        };
        match (init_n, step_n) {
            (Num::Int(i0), Num::Int(st)) => {
                if st == 0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
                // integer loop; a float limit clips to the integer range
                let (lim, empty) = int_for_limit(limit_n, i0, st);
                if empty {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                let count = if st > 0 {
                    (lim as u64).wrapping_sub(i0 as u64) / (st as u64)
                } else {
                    (i0 as u64).wrapping_sub(lim as u64) / (st as i128).unsigned_abs() as u64
                };
                self.set_r(base, a, Value::Int(i0));
                self.set_r(base, a + 1, Value::Int(count as i64));
                self.set_r(base, a + 2, Value::Int(st));
                self.set_r(base, a + 3, Value::Int(i0));
            }
            _ => {
                let (x0, lim, st) = (init_n.as_f64(), limit_n.as_f64(), step_n.as_f64());
                if st == 0.0 {
                    return Err(self.rt_err("'for' step is zero"));
                }
                let runs = if st > 0.0 { x0 <= lim } else { x0 >= lim };
                if !runs {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                self.set_r(base, a, Value::Float(x0));
                self.set_r(base, a + 1, Value::Float(lim));
                self.set_r(base, a + 2, Value::Float(st));
                self.set_r(base, a + 3, Value::Float(x0));
            }
        }
        Ok(())
    }

    fn for_loop(&mut self, inst: Inst, base: u32) {
        let a = inst.a();
        match self.r(base, a) {
            Value::Int(cur) => {
                let Value::Int(count) = self.r(base, a + 1) else {
                    unreachable!()
                };
                if count > 0 {
                    let Value::Int(st) = self.r(base, a + 2) else {
                        unreachable!()
                    };
                    let next = cur.wrapping_add(st);
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 1, Value::Int(count - 1));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            Value::Float(cur) => {
                let Value::Float(lim) = self.r(base, a + 1) else {
                    unreachable!()
                };
                let Value::Float(st) = self.r(base, a + 2) else {
                    unreachable!()
                };
                let next = cur + st;
                let cont = if st > 0.0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Float(next));
                    self.set_r(base, a + 3, Value::Float(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            _ => unreachable!("corrupt for-loop state"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArithOp {
    Add,
    Sub,
    Mul,
    Mod,
    Pow,
    Div,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

fn as_num(v: Value) -> Option<Num> {
    match v {
        Value::Int(i) => Some(Num::Int(i)),
        Value::Float(f) => Some(Num::Float(f)),
        _ => None,
    }
}

/// Lua shifts: logical on 64 bits; |shift| ≥ 64 yields 0; negative shifts
/// reverse direction.
fn shift_left(a: i64, b: i64) -> i64 {
    if b < 0 {
        if b <= -64 {
            0
        } else {
            ((a as u64) >> (-b as u32)) as i64
        }
    } else if b >= 64 {
        0
    } else {
        ((a as u64) << (b as u32)) as i64
    }
}

/// i < f, exactly (PUC LTintfloat shape).
fn int_lt_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    let ff = f.floor();
    let fi = ff as i64;
    if f == ff { i < fi } else { i <= fi }
}

/// i <= f, exactly.
fn int_le_float(i: i64, f: f64) -> bool {
    if f.is_nan() {
        return false;
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return true;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return false;
    }
    i <= f.floor() as i64
}

/// Clip a numeric `for` limit to the integer range (PUC forlimit). Returns
/// (clipped limit, loop-is-empty).
fn int_for_limit(limit: Num, init: i64, step: i64) -> (i64, bool) {
    match limit {
        Num::Int(l) => {
            let empty = if step > 0 { init > l } else { init < l };
            (l, empty)
        }
        Num::Float(f) => {
            if f.is_nan() {
                return (0, true);
            }
            if step > 0 {
                if f >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, false)
                } else {
                    let l = f.floor();
                    if l < -9_223_372_036_854_775_808.0 {
                        (i64::MIN, true)
                    } else {
                        let li = l as i64;
                        (li, init > li)
                    }
                }
            } else if f <= -9_223_372_036_854_775_808.0 {
                (i64::MIN, false)
            } else {
                let l = f.ceil();
                if l >= 9_223_372_036_854_775_808.0 {
                    (i64::MAX, init < i64::MAX)
                } else {
                    let li = l as i64;
                    (li, init < li)
                }
            }
        }
    }
}

/// Strip the load-prefix sigil from a chunk name for messages (PUC keeps
/// `@file` / `=name` markers in `source`).
fn numeric_src_name(p: *const crate::runtime::LuaStr) -> &'static [u8] {
    let b = unsafe { crate::runtime::string::bytes_of(p) };
    match b.first() {
        Some(b'@') | Some(b'=') => &b[1..],
        _ => b,
    }
}
