// CARVE-OUT: opcode dispatch table, one arm per fast opcode
//! The interpreter's fast loop: the opcodes that never call, allocate or
//! run a metamethod execute here on locals, in a function of their own so
//! that its register allocation does not depend on the rest of the loop.

use super::*;

/// `R[A] := R[B] op R[C]`: two integers and two floats are computed in the
/// opcode arm, a float meeting an integer as two floats (an arm yielding
/// `None` falls through); everything else goes to `arith_slow`. `true` when
/// the opcode arm finished the operation.
macro_rules! arith_arm {
    ($vm:ident, $regs:ident, $inst:ident, $base:ident, $op:expr,
     int($ia:ident, $ib:ident) => $iv:expr, float($fa:ident, $fb:ident) => $fv:expr) => {{
        // SAFETY: `$regs` is the running frame's register window (see `Vm::r`)
        let l = unsafe { *$regs.add($inst.b() as usize) };
        // SAFETY: as above
        let r = unsafe { *$regs.add($inst.c() as usize) };
        let v: Option<Value> = match (l, r) {
            (Value::Int($ia), Value::Int($ib)) => $iv,
            (Value::Float($fa), Value::Float($fb)) => $fv,
            (Value::Float($fa), Value::Int(i)) => {
                let $fb = i as f64;
                $fv
            }
            (Value::Int(i), Value::Float($fb)) => {
                let $fa = i as f64;
                $fv
            }
            _ => None,
        };
        match v {
            Some(v) => {
                // SAFETY: as above
                unsafe { *$regs.add($inst.a() as usize) = v };
                true
            }
            None => {
                $vm.arith_slow($inst.a(), $base, $op, l, r, $inst.k())?;
                false
            }
        }
    }};
}

/// `R[A] := R[B] op c` for a constant or immediate `c`: as `arith_arm`, and
/// a float meeting an integer is computed as two floats. The slow path gets
/// the operands in source order (`k`: the constant was on the left).
macro_rules! arith_c_arm {
    ($vm:ident, $regs:ident, $inst:ident, $base:ident, $op:expr, $c:expr,
     int($ia:ident, $ib:ident) => $iv:expr, float($fa:ident, $fb:ident) => $fv:expr) => {{
        // SAFETY: `$regs` is the running frame's register window (see `Vm::r`)
        let x = unsafe { *$regs.add($inst.b() as usize) };
        let c: Value = $c;
        let v: Option<Value> = match (x, c) {
            (Value::Int($ia), Value::Int($ib)) => $iv,
            (Value::Float($fa), Value::Float($fb)) => $fv,
            (Value::Float($fa), Value::Int(i)) => {
                let $fb = i as f64;
                $fv
            }
            (Value::Int(i), Value::Float($fb)) => {
                let $fa = i as f64;
                $fv
            }
            _ => None,
        };
        match v {
            Some(v) => {
                // SAFETY: as above
                unsafe { *$regs.add($inst.a() as usize) = v };
                true
            }
            None => {
                let (l, r) = if $inst.k() { (c, x) } else { (x, c) };
                $vm.arith_slow($inst.a(), $base, $op, l, r, false)?;
                false
            }
        }
    }};
}

/// The running frame, as the loop head found it.
pub(super) struct Fast {
    pub(super) cl: Gc<LuaClosure>,
    pub(super) base: u32,
    pub(super) func_slot: u32,
    pub(super) n_varargs: u32,
    /// the frame's pc field
    pub(super) fpc: *mut u32,
    pub(super) trace_on: bool,
    pub(super) pre53: bool,
    pub(super) entry_depth: usize,
}

/// Why the fast loop handed control back.
pub(super) enum FastExit {
    /// the frame state may have changed: back to the loop head
    Reload,
    /// an opcode the loop head's own match runs
    Slow(Inst),
}

impl Vm {
    /// Run instructions from `inst` (at `npc - 1`) until one needs the loop
    /// head. The frame's pc is `npc` on entry and is kept current.
    #[inline(never)]
    pub(super) fn run_fast(
        &mut self,
        fx: Fast,
        mut inst: Inst,
        mut npc: u32,
    ) -> Result<FastExit, LuaError> {
        let v54 = self.version() >= LuaVersion::Lua54;
        let Fast {
            mut cl,
            mut base,
            mut func_slot,
            mut n_varargs,
            mut fpc,
            trace_on,
            pre53,
            entry_depth,
        } = fx;
        // From here the running frame's state lives in locals (PUC keeps
        // `pc`, `base` and `k` in registers the same way). An arm that only
        // reads and writes registers advances `npc` and stores it through
        // `fpc`, which keeps the frame's pc current for errors and for
        // everything that reads it. An arm that may run Lua code, move the
        // stack or change frames goes through `resume!` or `reenter!`
        // afterwards; a metamethod call always pushes a continuation, which
        // sets `trap`, and then the loop head takes over (PUC `Protect`).
        // The loop keeps fetching here while no instruction needs the head's
        // checks: no trap, no recording and no compiled trace this function
        // could enter.
        let mut code = cl.proto.code.as_ptr();
        let mut kptr = cl.proto.consts.as_ptr();
        let mut klen = cl.proto.consts.len();
        // nothing in a fast arm sets `trap`, so it is tested here once.
        // With a trace this function could enter, the fast arms stop at
        // the pcs where one starts, for the dispatcher to look.
        let mut heads = [crate::runtime::function::TRACE_HEADS_NONE; 2];
        let stay = !self.trap
            && (!trace_on
                || self.jit.active_trace.is_none() && {
                    heads = cl.proto.trace_heads.get();
                    heads[0] != crate::runtime::function::TRACE_HEADS_MANY
                });
        // one test per instruction in the common case
        let plain = stay && heads[0] == crate::runtime::function::TRACE_HEADS_NONE;
        // the register window, valid while `self.stack` neither moves nor
        // is written through a reference
        // SAFETY: `push_frame` sized the stack to `base + max_stack`
        let mut regs: *mut Value = unsafe { self.stack.as_mut_ptr().add(base as usize) };
        macro_rules! reg {
            ($i:expr) => {
                // SAFETY: registers are below `max_stack` (see `Vm::r`)
                unsafe { *regs.add(($i) as usize) }
            };
        }
        macro_rules! set_reg {
            ($i:expr, $v:expr) => {
                // SAFETY: as for `reg!`
                unsafe { *regs.add(($i) as usize) = $v }
            };
        }
        macro_rules! konst {
            ($i:expr) => {
                // SAFETY: `kptr` / `klen` are the running proto's constants
                unsafe { std::slice::from_raw_parts(kptr, klen) }
                [($i) as usize]
            };
        }
        macro_rules! next {
            () => {{
                if !plain && (!stay || npc == heads[0] || npc == heads[1]) {
                    // SAFETY: `fpc` points into the running frame, which no
                    // fast arm moves
                    unsafe { *fpc = npc };
                    return Ok(FastExit::Reload);
                }
                // SAFETY: as for the fetch at the loop head
                inst = unsafe { *code.add(npc as usize) };
                npc += 1;
                // SAFETY: see above
                unsafe { *fpc = npc };
                continue;
            }};
        }
        // the top frame, known to be a Lua frame while `trap` is clear
        macro_rules! top_lua {
            () => {{
                debug_assert!(!self.trap);
                // SAFETY: the running thread has a frame, and with `trap`
                // clear it is not a continuation (see `frames_pop_sync`)
                match unsafe { self.frames.last_mut().unwrap_unchecked() } {
                    CallFrame::Lua(f) => f,
                    // SAFETY: see above
                    CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
                }
            }};
        }
        // after a slow path that ran no Lua code: the same frame, whose pc
        // it may have moved (a comparison's skip) and whose stack it may
        // have written
        macro_rules! resume {
            () => {{
                if self.trap {
                    return Ok(FastExit::Reload);
                }
                let f = top_lua!();
                npc = f.pc;
                fpc = &mut f.pc;
                // SAFETY: as at the start
                regs = unsafe { self.stack.as_mut_ptr().add(base as usize) };
                next!()
            }};
        }
        // after a call or return: take on whatever frame is now on top
        macro_rules! reenter {
            () => {{
                if self.trap {
                    return Ok(FastExit::Reload);
                }
                let f = top_lua!();
                cl = f.closure;
                base = f.base;
                func_slot = f.func_slot;
                n_varargs = f.n_varargs;
                npc = f.pc;
                fpc = &mut f.pc;
                code = cl.proto.code.as_ptr();
                kptr = cl.proto.consts.as_ptr();
                klen = cl.proto.consts.len();
                // stay only between frames with nothing to watch, so that
                // `plain`, `stay` and `heads` hold for the whole loop
                if !plain
                    || trace_on
                        && (self.jit.active_trace.is_some()
                            || cl.proto.trace_heads.get()[0]
                                != crate::runtime::function::TRACE_HEADS_NONE)
                {
                    // the loop head looks at this pc first
                    return Ok(FastExit::Reload);
                }
                // SAFETY: as at the start
                regs = unsafe { self.stack.as_mut_ptr().add(base as usize) };
                // SAFETY: as for the fetch at the loop head
                inst = unsafe { *code.add(npc as usize) };
                npc += 1;
                // SAFETY: see above
                unsafe { *fpc = npc };
                continue;
            }};
        }
        loop {
            // the instruction now running
            let pc = npc - 1;
            match inst.op() {
                Op::Move => {
                    let v = reg!(inst.b());
                    set_reg!(inst.a(), v);
                    next!()
                }
                Op::LoadI => {
                    set_reg!(inst.a(), Value::Int(inst.sbx() as i64));
                    next!()
                }
                Op::LoadF => {
                    set_reg!(inst.a(), Value::Float(inst.sbx() as f64));
                    next!()
                }
                Op::LoadK => {
                    let v = konst!(inst.bx());
                    set_reg!(inst.a(), v);
                    next!()
                }
                Op::LoadFalse => {
                    set_reg!(inst.a(), Value::Bool(false));
                    next!()
                }
                Op::LFalseSkip => {
                    set_reg!(inst.a(), Value::Bool(false));
                    npc += 1;
                    next!()
                }
                Op::LoadTrue => {
                    set_reg!(inst.a(), Value::Bool(true));
                    next!()
                }
                Op::LoadNil => {
                    let a = inst.a();
                    for i in 0..=inst.b() {
                        set_reg!(a + i, Value::Nil);
                    }
                    next!()
                }
                Op::GetUpval => {
                    let v = self.upval_get(cl, inst.b());
                    set_reg!(inst.a(), v);
                    next!()
                }
                Op::SetUpval => {
                    let v = reg!(inst.a());
                    self.upval_set(cl, inst.b(), v);
                    // the write may have gone through `self.stack`
                    // SAFETY: as at the loop head
                    regs = unsafe { self.stack.as_mut_ptr().add(base as usize) };
                    next!()
                }
                Op::GetTabUp => {
                    let t = self.upval_get(cl, inst.b());
                    let key = konst!(inst.c());
                    let dst = base + inst.a();
                    if let Some(v) = self.index_raw(t, key) {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    self.index_miss(t, key, dst)?;
                }
                Op::GetTable => {
                    let t = reg!(inst.b());
                    let key = reg!(inst.c());
                    let dst = base + inst.a();
                    if let Some(v) = self.index_raw(t, key) {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    self.index_miss(t, key, dst)?;
                }
                Op::GetI => {
                    let t = reg!(inst.b());
                    let key = Value::Int(inst.c() as i64);
                    let dst = base + inst.a();
                    if let Some(v) = self.index_raw(t, key) {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    self.index_miss(t, key, dst)?;
                }
                Op::GetField => {
                    let t = reg!(inst.b());
                    let key = konst!(inst.c());
                    let dst = base + inst.a();
                    if let Some(v) = self.index_raw(t, key) {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    self.index_miss(t, key, dst)?;
                }
                Op::SetTabUp => {
                    let t = self.upval_get(cl, inst.a());
                    let key = konst!(inst.b());
                    let v = reg!(inst.c());
                    if self.newindex_raw(t, key, v) {
                        next!()
                    }
                    self.newindex_miss(t, key, v)?;
                }
                Op::SetTable => {
                    let t = reg!(inst.a());
                    let key = reg!(inst.b());
                    let v = reg!(inst.c());
                    if self.newindex_raw(t, key, v) {
                        next!()
                    }
                    self.newindex_miss(t, key, v)?;
                }
                Op::SetI => {
                    let t = reg!(inst.a());
                    let key = Value::Int(inst.b() as i64);
                    let v = reg!(inst.c());
                    if self.newindex_raw(t, key, v) {
                        next!()
                    }
                    self.newindex_miss(t, key, v)?;
                }
                Op::SetField => {
                    let t = reg!(inst.a());
                    let key = konst!(inst.b());
                    let v = reg!(inst.c());
                    if self.newindex_raw(t, key, v) {
                        next!()
                    }
                    self.newindex_miss(t, key, v)?;
                }
                Op::SelfOp => {
                    let o = reg!(inst.b());
                    set_reg!(inst.a() + 1, o);
                    // PUC OP_SELF's C is a constant index when the k-flag is
                    // set; otherwise it points to a register that holds the
                    // (constant-loaded) key. luna's compiler falls back to the
                    // register form when the constant index exceeds OP_SELF's
                    // 8-bit C field (5.1 big.lua's `a:findfield(...)` against
                    // a table with 250+ string keys, where "findfield" lands
                    // past const #255). The exec must honour the same split.
                    let key = if inst.k() {
                        konst!(inst.c())
                    } else {
                        reg!(inst.c())
                    };
                    let dst = base + inst.a();
                    if let Some(v) = self.index_raw(o, key) {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    self.index_miss(o, key, dst)?;
                }
                Op::Add => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Add,
                    int(a, b) => Some(Value::Int(a.wrapping_add(b))),
                    float(a, b) => Some(Value::Float(a + b)))
                    {
                        next!()
                    }
                }
                Op::Sub => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Sub,
                    int(a, b) => Some(Value::Int(a.wrapping_sub(b))),
                    float(a, b) => Some(Value::Float(a - b)))
                    {
                        next!()
                    }
                }
                Op::Mul => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Mul,
                    int(a, b) => Some(Value::Int(a.wrapping_mul(b))),
                    float(a, b) => Some(Value::Float(a * b)))
                    {
                        next!()
                    }
                }
                // a zero divisor takes the slow path for its error
                Op::Mod => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Mod,
                    int(a, b) => (b != 0).then(|| Value::Int(int_mod(a, b))),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::IDiv => {
                    if arith_arm!(self, regs, inst, base, ArithOp::IDiv,
                    int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::Div => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Div,
                    int(a, b) => Some(Value::Float(a as f64 / b as f64)),
                    float(a, b) => Some(Value::Float(a / b)))
                    {
                        next!()
                    }
                }
                Op::BAnd => {
                    if arith_arm!(self, regs, inst, base, ArithOp::BAnd,
                    int(a, b) => Some(Value::Int(a & b)),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::BOr => {
                    if arith_arm!(self, regs, inst, base, ArithOp::BOr,
                    int(a, b) => Some(Value::Int(a | b)),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::BXor => {
                    if arith_arm!(self, regs, inst, base, ArithOp::BXor,
                    int(a, b) => Some(Value::Int(a ^ b)),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::Shl => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Shl,
                    int(a, b) => Some(Value::Int(shift_left(a, b))),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::Shr => {
                    if arith_arm!(self, regs, inst, base, ArithOp::Shr,
                    int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))),
                    float(_a, _b) => None)
                    {
                        next!()
                    }
                }
                Op::AddI => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Add, Value::Int(inst.sc() as i64),
                        int(a, b) => Some(Value::Int(a.wrapping_add(b))),
                        float(a, b) => Some(Value::Float(a + b)))
                    {
                        next!()
                    }
                }
                Op::SubI => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Sub, Value::Int(inst.sc() as i64),
                        int(a, b) => Some(Value::Int(a.wrapping_sub(b))),
                        float(a, b) => Some(Value::Float(a - b)))
                    {
                        next!()
                    }
                }
                Op::AddK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Add, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a.wrapping_add(b))),
                        float(a, b) => Some(Value::Float(a + b)))
                    {
                        next!()
                    }
                }
                Op::SubK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Sub, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a.wrapping_sub(b))),
                        float(a, b) => Some(Value::Float(a - b)))
                    {
                        next!()
                    }
                }
                Op::MulK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Mul, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a.wrapping_mul(b))),
                        float(a, b) => Some(Value::Float(a * b)))
                    {
                        next!()
                    }
                }
                // a zero divisor takes the slow path for its error
                Op::ModK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Mod, konst!(inst.c()),
                        int(a, b) => (b != 0).then(|| Value::Int(int_mod(a, b))),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::IDivK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::IDiv, konst!(inst.c()),
                        int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::DivK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Div, konst!(inst.c()),
                        int(a, b) => Some(Value::Float(a as f64 / b as f64)),
                        float(a, b) => Some(Value::Float(a / b)))
                    {
                        next!()
                    }
                }
                Op::PowK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Pow, konst!(inst.c()),
                        int(a, b) => Some(Value::Float(num_pow(v54, a as f64, b as f64))),
                        float(a, b) => Some(Value::Float(num_pow(v54, a, b))))
                    {
                        next!()
                    }
                }
                Op::BAndK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::BAnd, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a & b)),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::BOrK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::BOr, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a | b)),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::BXorK => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::BXor, konst!(inst.c()),
                        int(a, b) => Some(Value::Int(a ^ b)),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::ShrI => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Shr, Value::Int(inst.sc() as i64),
                        int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::ShlI => {
                    if arith_c_arm!(self, regs, inst, base, ArithOp::Shl, Value::Int(inst.sc() as i64),
                        int(a, b) => Some(Value::Int(shift_left(a, b))),
                        float(a, b) => { let _ = (a, b); None })
                    {
                        next!()
                    }
                }
                Op::Unm => {
                    let v = reg!(inst.b());
                    match self.unary_operand(v) {
                        Some(Num::Int(i)) => {
                            set_reg!(inst.a(), Value::Int(i.wrapping_neg()));
                            next!()
                        }
                        Some(Num::Float(f)) => {
                            set_reg!(inst.a(), Value::Float(-f));
                            next!()
                        }
                        None => {
                            let mm = self.get_mm(v, Mm::Unm);
                            if mm.is_nil() {
                                return Err(self.type_err("perform arithmetic on", v));
                            }
                            let dst = base + inst.a();
                            self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                        }
                    }
                }
                Op::BNot => {
                    let v = reg!(inst.b());
                    match self.arith_operand()(v) {
                        Some(n) => {
                            let Some(i) = int_of(n) else {
                                return Err(self.no_int_rep_err());
                            };
                            set_reg!(inst.a(), Value::Int(!i));
                            next!()
                        }
                        None => {
                            let mm = self.get_mm(v, Mm::BNot);
                            if mm.is_nil() {
                                return Err(self.type_err("perform bitwise operation on", v));
                            }
                            let dst = base + inst.a();
                            self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                        }
                    }
                }
                Op::Not => {
                    let v = reg!(inst.b());
                    set_reg!(inst.a(), Value::Bool(!v.truthy()));
                    next!()
                }
                Op::Len => {
                    let v = reg!(inst.b());
                    // no `__len` to look for: a string, or a table without
                    // a metatable
                    match v {
                        Value::Str(s) => {
                            set_reg!(inst.a(), Value::Int(s.len() as i64));
                            next!()
                        }
                        Value::Table(t) if t.metatable().is_none() => {
                            set_reg!(inst.a(), Value::Int(t.len()));
                            next!()
                        }
                        _ => {}
                    }
                    match self.len_step(v)? {
                        MmOut::Done(r) => self.set_r(base, inst.a(), r),
                        MmOut::Mm { func, recv } => {
                            let dst = base + inst.a();
                            self.begin_meta_call(func, &[recv, recv], MetaAction::Store { dst })?;
                        }
                        MmOut::CompareSynth { .. } => {
                            unreachable!("CompareSynth from len_step")
                        }
                    }
                }
                Op::Jmp => {
                    let off = inst.sj();
                    npc = (npc as i64 + off as i64) as u32;
                    // a backward jump is a loop's back-edge: the trace JIT
                    // counts them, and from the threshold on looks whether
                    // to record from the target
                    if trace_on && off < 0 {
                        let proto = cl.proto;
                        let c = proto.trace_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.trace_hot_count.set(c + 1);
                        }
                        let target = (pc as i32 + 1 + off).max(0) as u32;
                        if c >= self.jit.trace_hot_threshold
                            && self.trace_start_at_jmp(cl, base, target)
                        {
                            // the recording sees the next instruction from
                            // the loop head
                            // SAFETY: see `next!`
                            unsafe { *fpc = npc };
                            return Ok(FastExit::Reload);
                        }
                    }
                    next!()
                }
                Op::Eq => {
                    let l = reg!(inst.a());
                    let r = reg!(inst.b());
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a == b) != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    let step = self.eq_step(l, r);
                    self.op_compare(step, l, r, inst.k())?;
                }
                Op::EqK => {
                    let l = reg!(inst.a());
                    let r = konst!(inst.b());
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a == b) != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    let step = self.eq_step(l, r);
                    self.op_compare(step, l, r, inst.k())?;
                }
                Op::Lt => {
                    let l = reg!(inst.a());
                    let r = reg!(inst.b());
                    // hot path: Int < Int — drops the MmOut + op_compare match
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a < b) != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    let step = self.less_step(l, r, false)?;
                    self.op_compare(step, l, r, inst.k())?;
                }
                Op::Le => {
                    let l = reg!(inst.a());
                    let r = reg!(inst.b());
                    if let (Value::Int(a), Value::Int(b)) = (l, r) {
                        if (a <= b) != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    let step = self.less_step(l, r, true)?;
                    self.op_compare(step, l, r, inst.k())?;
                }
                // raw equality with a number: no metamethod can be involved
                Op::EqI => {
                    let im = inst.sb();
                    let eq = match reg!(inst.a()) {
                        Value::Int(a) => a == im as i64,
                        Value::Float(f) => f == im as f64,
                        _ => false,
                    };
                    if eq != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::LtI => {
                    let x = reg!(inst.a());
                    let im = inst.sb();
                    let res = match x {
                        Value::Int(a) => a < im as i64,
                        Value::Float(f) => f < im as f64,
                        _ => {
                            let imv = if inst.c() != 0 {
                                Value::Float(im as f64)
                            } else {
                                Value::Int(im as i64)
                            };
                            let step = self.less_step(x, imv, false)?;
                            self.op_compare(step, x, imv, inst.k())?;
                            resume!()
                        }
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::LeI => {
                    let x = reg!(inst.a());
                    let im = inst.sb();
                    let res = match x {
                        Value::Int(a) => a <= im as i64,
                        Value::Float(f) => f <= im as f64,
                        _ => {
                            let imv = if inst.c() != 0 {
                                Value::Float(im as f64)
                            } else {
                                Value::Int(im as i64)
                            };
                            let step = self.less_step(x, imv, true)?;
                            self.op_compare(step, x, imv, inst.k())?;
                            resume!()
                        }
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::GtI => {
                    let x = reg!(inst.a());
                    let im = inst.sb();
                    let res = match x {
                        Value::Int(a) => a > im as i64,
                        Value::Float(f) => f > im as f64,
                        _ => {
                            let imv = if inst.c() != 0 {
                                Value::Float(im as f64)
                            } else {
                                Value::Int(im as i64)
                            };
                            let step = self.less_step(imv, x, false)?;
                            self.op_compare(step, imv, x, inst.k())?;
                            resume!()
                        }
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::GeI => {
                    let x = reg!(inst.a());
                    let im = inst.sb();
                    let res = match x {
                        Value::Int(a) => a >= im as i64,
                        Value::Float(f) => f >= im as f64,
                        _ => {
                            let imv = if inst.c() != 0 {
                                Value::Float(im as f64)
                            } else {
                                Value::Int(im as i64)
                            };
                            let step = self.less_step(imv, x, true)?;
                            self.op_compare(step, imv, x, inst.k())?;
                            resume!()
                        }
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::Test => {
                    // the JMP that follows runs when the condition equals k
                    if reg!(inst.a()).truthy() != inst.k() {
                        npc += 1;
                    }
                    next!()
                }
                Op::TestSet => {
                    let v = reg!(inst.b());
                    if v.truthy() == inst.k() {
                        set_reg!(inst.a(), v);
                    } else {
                        npc += 1;
                    }
                    next!()
                }
                Op::ForLoop => {
                    // `for_loop` is the reference: 5.1–5.3 step and compare with
                    // the limit, 5.4+ count down; anything else it raises on
                    let a = inst.a();
                    let back = npc.wrapping_sub(inst.bx());
                    let mut slow = false;
                    match (reg!(a), reg!(a + 1), reg!(a + 2)) {
                        (Value::Int(cur), Value::Int(count), Value::Int(st)) if !pre53 => {
                            if count != 0 {
                                let next = cur.wrapping_add(st);
                                set_reg!(a, Value::Int(next));
                                set_reg!(a + 1, Value::Int(count.wrapping_sub(1)));
                                set_reg!(a + 3, Value::Int(next));
                                npc = back;
                            }
                        }
                        (Value::Int(cur), Value::Int(lim), Value::Int(st)) if pre53 => {
                            let next = cur.wrapping_add(st);
                            if if st > 0 { next <= lim } else { next >= lim } {
                                set_reg!(a, Value::Int(next));
                                set_reg!(a + 3, Value::Int(next));
                                npc = back;
                            }
                        }
                        (Value::Float(cur), Value::Float(lim), Value::Float(st)) => {
                            let next = cur + st;
                            if if st > 0.0 { next <= lim } else { next >= lim } {
                                set_reg!(a, Value::Float(next));
                                set_reg!(a + 3, Value::Float(next));
                                npc = back;
                            }
                        }
                        _ => {
                            self.for_loop(inst, base)?;
                            npc = self.top_frame().pc;
                            slow = true;
                        }
                    }
                    // The trace JIT counts the back-edges taken and starts
                    // recording at the body once the count reaches the
                    // threshold.
                    if trace_on && npc != pc + 1 {
                        let proto = cl.proto;
                        let c = proto.trace_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.trace_hot_count.set(c + 1);
                        }
                        if c == self.jit.trace_hot_threshold && self.jit.active_trace.is_none() {
                            // the back-edge target is the body's first op
                            let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                            self.trace_start_at_loop(cl, base, target, None);
                            slow = true;
                        }
                    }
                    if slow {
                        // SAFETY: see `next!`
                        unsafe { *fpc = npc };
                        return Ok(FastExit::Reload);
                    }
                    next!()
                }
                Op::TForLoop => {
                    let a = inst.a();
                    let ctrl = reg!(a + 4);
                    if !ctrl.is_nil() {
                        // the generic-for's back-edge, counted like a
                        // numeric one; an iterator that returned nothing
                        // takes no back-edge
                        if trace_on {
                            let proto = cl.proto;
                            let c = proto.trace_hot_count.get();
                            if c < u32::MAX / 2 {
                                proto.trace_hot_count.set(c + 1);
                            }
                            if c == self.jit.trace_hot_threshold && self.jit.active_trace.is_none()
                            {
                                // the body's first op, right after TForPrep
                                let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                                self.trace_start_at_loop(cl, base, target, Some(a));
                            }
                        }
                        set_reg!(a + 2, ctrl);
                        npc = npc.wrapping_sub(inst.bx());
                        // a recording that just started must see the next
                        // instruction from the loop head
                        if trace_on && self.jit.active_trace.is_some() {
                            // SAFETY: see `next!`
                            unsafe { *fpc = npc };
                            return Ok(FastExit::Reload);
                        }
                    }
                    next!()
                }
                Op::VargIdx => {
                    // R[A] := vararg[R[C]] without allocating: integer key in
                    // [1,n] → that vararg, "n" → the count, else nil.
                    let key = reg!(inst.c());
                    let n = n_varargs;
                    let v = match key {
                        Value::Int(k) if k >= 1 && (k as u64) <= n as u64 => {
                            self.stack[(func_slot + k as u32) as usize]
                        }
                        Value::Float(f) if f.fract() == 0.0 && f >= 1.0 && f <= n as f64 => {
                            self.stack[(func_slot + f as u32) as usize]
                        }
                        Value::Str(s) if s.as_bytes() == b"n" => Value::Int(n as i64),
                        _ => Value::Nil,
                    };
                    set_reg!(inst.a(), v);
                    next!()
                }
                Op::ErrNNil => {
                    let v = self.r(base, inst.a());
                    if !matches!(v, Value::Nil) {
                        let bx = inst.bx();
                        let name = if bx == 0 {
                            "?".to_string()
                        } else {
                            match cl.proto.consts[(bx - 1) as usize] {
                                Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                                _ => "?".to_string(),
                            }
                        };
                        return Err(self.rt_err(&format!("global '{name}' already defined")));
                    }
                    next!()
                }
                Op::Call => {
                    let abs = base + inst.a();
                    let nargs = if inst.b() == 0 {
                        None
                    } else {
                        Some(inst.b() - 1)
                    };
                    let wanted = inst.c() as i32 - 1;
                    self.begin_call(abs, nargs, wanted, false)?;
                    reenter!()
                }
                // the common returns: to a Lua caller in this activation, with
                // nothing to close and no hook (see `return_to_lua`); the
                // loop head's `Return` arm does the rest
                Op::Return0 | Op::Return1 => {
                    let (abs_a, nret) = if inst.op() == Op::Return0 {
                        (base, 0)
                    } else {
                        (base + inst.a(), 1)
                    };
                    self.top = self.top.max(abs_a + nret);
                    if self.return_to_lua(base, abs_a, nret, entry_depth) {
                        reenter!()
                    }
                    return Ok(FastExit::Slow(inst));
                }
                // listed rather than `_`, so that the jump table covers every
                // opcode without a range check
                Op::LoadKx
                | Op::NewTable
                | Op::SetList
                | Op::Pow
                | Op::Concat
                | Op::Close
                | Op::Tbc
                | Op::TailCall
                | Op::Return
                | Op::ForPrep
                | Op::TForPrep
                | Op::TForCall
                | Op::Closure
                | Op::Vararg
                | Op::GetVarg
                | Op::ExtraArg => return Ok(FastExit::Slow(inst)),
            }
            // a fast arm's slow path
            resume!()
        }
    }
}
