// CARVE-OUT: opcode dispatch table, one arm per fast opcode
//! The interpreter's fast loop: the opcodes that never call, allocate or
//! run a metamethod execute here on locals, in a function of their own so
//! that its register allocation does not depend on the rest of the loop.

use super::*;
use crate::runtime::value::tag;
use fast_arith::{
    arith_arm, arith_imm_arm, put_int, raw_flt, raw_gc, raw_int, raw_tag, raw_truthy,
};

/// The running frame, as the loop head found it.
pub(super) struct Fast {
    /// the running frame, on top of `frames`
    pub(super) fr: *mut Frame,
    pub(super) trace_on: bool,
    pub(super) pre53: bool,
    pub(super) entry_depth: usize,
    /// false when some instruction needs the loop head's checks (a trap,
    /// a recording, or more trace heads than `heads` holds)
    pub(super) stay: bool,
    /// the pcs where a trace this function could enter starts
    pub(super) heads: [u32; 2],
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
    /// head. The frame's pc is `npc` on entry and is kept current. `WATCH`
    /// is false when `fx.stay` holds and `fx.heads` is empty: that loop then
    /// tests nothing per instruction.
    #[inline(never)]
    pub(super) fn run_fast<const WATCH: bool>(
        &mut self,
        fx: Fast,
        mut inst: Inst,
        mut npc: u32,
    ) -> Result<FastExit, LuaError> {
        let Fast {
            mut fr,
            trace_on,
            pre53,
            entry_depth,
            stay,
            heads,
        } = fx;
        // From here the running frame's state lives in locals (PUC keeps
        // `pc`, `base` and `k` in registers the same way). An arm that only
        // reads and writes registers advances `npc` and stores it through
        // `fr`, which keeps the frame's pc current for errors and for
        // everything that reads it. An arm that may run Lua code, move the
        // stack or change frames goes through `resume!` or `reenter!`
        // afterwards; a metamethod call always pushes a continuation, which
        // sets `trap`, and then the loop head takes over (PUC `Protect`).
        // The loop keeps fetching here while no instruction needs the head's
        // checks: no trap, no recording and no compiled trace this function
        // could enter.
        // `fr` points into `self.frames`: it is taken again after anything
        // that can push a frame. The frame's other fields are read through
        // it where an arm needs them, which keeps them out of the loop's
        // registers.
        macro_rules! cl {
            () => {
                // SAFETY: `fr` is the running frame
                unsafe { (*fr).closure }
            };
        }
        macro_rules! base {
            () => {
                // SAFETY: as above
                unsafe { (*fr).base }
            };
        }
        let mut code = cl!().proto.code.as_ptr();
        let mut kptr = cl!().proto.consts.as_ptr();
        // the register window, valid while `self.stack` neither moves nor
        // is written through a reference
        // SAFETY: `push_frame` sized the stack to `base + max_stack`
        let base = base!();
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
                // SAFETY: the compiler and the bytecode verifier keep
                // constant indices below the proto's constant count
                unsafe { *kptr.add(($i) as usize) }
            };
        }
        macro_rules! next {
            () => {{
                // nothing in a fast arm sets `trap`, so `stay` holds for the
                // whole loop; with a trace this function could enter, the
                // arms stop at the pcs where one starts, for the dispatcher
                if WATCH && (!stay || npc == heads[0] || npc == heads[1]) {
                    // SAFETY: `fr` is the running frame, which no fast arm
                    // moves
                    unsafe { (*fr).pc = npc };
                    return Ok(FastExit::Reload);
                }
                // SAFETY: as for the fetch at the loop head
                inst = unsafe { *code.add(npc as usize) };
                npc += 1;
                if WATCH {
                    // SAFETY: see above
                    unsafe { (*fr).pc = npc };
                }
                continue;
            }};
        }
        // Without anything to watch the frame's pc is stored only before
        // whatever can read it: a call, a metamethod, an error, the loop
        // head (PUC `savepc`). Every slow path below starts with this.
        macro_rules! save {
            () => {
                if !WATCH {
                    // SAFETY: `fr` is the running frame
                    unsafe { (*fr).pc = npc };
                }
            };
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
        // `'frames` starts over on whatever frame is on top after a call or
        // return; the first pass runs the frame the loop head handed over
        let mut switched = false;
        'frames: loop {
            if switched {
                let f = top_lua!();
                let cl = f.closure;
                npc = f.pc;
                let base = f.base;
                fr = f;
                code = cl.proto.code.as_ptr();
                kptr = cl.proto.consts.as_ptr();
                // stay only between frames with nothing to watch, so that
                // `stay` and `heads` hold for the whole loop
                if WATCH
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
            }
            switched = true;
            // after a call or return: take on whatever frame is now on top
            macro_rules! reenter {
                () => {{
                    if self.trap && !self.settle_frames(entry_depth)? {
                        return Ok(FastExit::Reload);
                    }
                    continue 'frames;
                }};
            }
            // after a slow path: the same frame, whose pc it may have moved (a
            // comparison's skip) and whose stack it may have written, unless
            // it called a metamethod
            macro_rules! resume {
                () => {{
                    if self.trap {
                        reenter!()
                    }
                    let f = top_lua!();
                    npc = f.pc;
                    let base = f.base;
                    fr = f;
                    // SAFETY: as at the start
                    regs = unsafe { self.stack.as_mut_ptr().add(base as usize) };
                    next!()
                }};
            }
            // `R[A] := R[B][*pk]` for a key in a register or a constant
            macro_rules! get_arm {
                ($pk:expr) => {{
                    let pt = regs.wrapping_add(inst.b() as usize);
                    let pk: *const Value = $pk;
                    // SAFETY: a register and a register or constant of the
                    // running frame
                    if let Some(v) = unsafe { Vm::index_raw_at(pt, pk) } {
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    save!();
                    // SAFETY: as above
                    let (t, key) = unsafe { (*pt, *pk) };
                    self.index_miss(t, key, base!() + inst.a())?;
                }};
            }
            // `R[A][*pk] := R[C]` for a key in a register or a constant
            macro_rules! set_arm {
                ($pk:expr) => {{
                    let pt = regs.wrapping_add(inst.a() as usize);
                    let pk: *const Value = $pk;
                    let v = reg!(inst.c());
                    // SAFETY: a register and a register or constant of the
                    // running frame
                    if unsafe { self.newindex_raw_at(pt, pk, v) } {
                        next!()
                    }
                    save!();
                    // SAFETY: as above
                    let (t, key) = unsafe { (*pt, *pk) };
                    self.newindex_miss(t, key, v)?;
                }};
            }
            // `R[A] < R[B]` / `<=` (PUC `op_order`): two integers or two
            // floats here, the rest (mixed numbers, strings, `__lt` / `__le`)
            // by `less_step`
            macro_rules! order_arm {
                ($op:tt, $or_eq:expr) => {{
                    let (pl, pr) = (
                        regs.wrapping_add(inst.a() as usize),
                        regs.wrapping_add(inst.b() as usize),
                    );
                    // SAFETY: registers of the running frame
                    let (tl, tr) = unsafe { (raw_tag(pl), raw_tag(pr)) };
                    let res = if tl == tag::INT && tr == tag::INT {
                        // SAFETY: two integers
                        (unsafe { raw_int(pl) }) $op (unsafe { raw_int(pr) })
                    } else if tl == tag::FLOAT && tr == tag::FLOAT {
                        // SAFETY: two floats
                        (unsafe { raw_flt(pl) }) $op (unsafe { raw_flt(pr) })
                    } else {
                        // SAFETY: as above
                        let (l, r) = unsafe { (*pl, *pr) };
                        save!();
                        let step = self.less_step(l, r, $or_eq)?;
                        self.op_compare(step, l, r, inst.k())?;
                        resume!()
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
                }};
            }
            // `R[A] op sB` (PUC `op_orderI`); `$swap`: the immediate is the
            // left operand of the metamethod (`>` and `>=`), and `C` says it
            // was written as a float
            macro_rules! order_imm_arm {
                ($op:tt, $swap:expr, $or_eq:expr) => {{
                    let px = regs.wrapping_add(inst.a() as usize);
                    let im = inst.sb();
                    // SAFETY: a register of the running frame
                    let t = unsafe { raw_tag(px) };
                    let res = if t == tag::INT {
                        // SAFETY: an integer
                        (unsafe { raw_int(px) }) $op (im as i64)
                    } else if t == tag::FLOAT {
                        // SAFETY: a float
                        (unsafe { raw_flt(px) }) $op (im as f64)
                    } else {
                        // SAFETY: as above
                        let x = unsafe { *px };
                        let imv = if inst.c() != 0 {
                            Value::Float(im as f64)
                        } else {
                            Value::Int(im as i64)
                        };
                        let (l, r) = if $swap { (imv, x) } else { (x, imv) };
                        save!();
                        let step = self.less_step(l, r, $or_eq)?;
                        self.op_compare(step, l, r, inst.k())?;
                        resume!()
                    };
                    if res != inst.k() {
                        npc += 1;
                    }
                    next!()
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
                        let v = self.upval_get(cl!(), inst.b());
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    Op::SetUpval => {
                        let v = reg!(inst.a());
                        self.upval_set(cl!(), inst.b(), v);
                        // the write may have gone through `self.stack`
                        let base = base!();
                        // SAFETY: as at the loop head
                        regs = unsafe { self.stack.as_mut_ptr().add(base as usize) };
                        next!()
                    }
                    Op::GetTabUp => {
                        let t = self.upval_get(cl!(), inst.b());
                        let pk = kptr.wrapping_add(inst.c() as usize);
                        // SAFETY: a constant of the running proto
                        if let Some(v) = unsafe { Vm::index_raw_key_at(t, pk) } {
                            set_reg!(inst.a(), v);
                            next!()
                        }
                        let key = konst!(inst.c());
                        save!();
                        self.index_miss(t, key, base!() + inst.a())?;
                    }
                    Op::GetTable => get_arm!(regs.wrapping_add(inst.c() as usize)),
                    Op::GetField => get_arm!(kptr.wrapping_add(inst.c() as usize)),
                    Op::GetI => {
                        let t = reg!(inst.b());
                        let key = Value::Int(inst.c() as i64);
                        let dst = base!() + inst.a();
                        if let Some(v) = self.index_raw(t, key) {
                            set_reg!(inst.a(), v);
                            next!()
                        }
                        save!();
                        self.index_miss(t, key, dst)?;
                    }
                    Op::SetTabUp => {
                        let t = self.upval_get(cl!(), inst.a());
                        let pk = kptr.wrapping_add(inst.b() as usize);
                        let v = reg!(inst.c());
                        // SAFETY: a constant of the running proto
                        if unsafe { self.newindex_raw_key_at(t, pk, v) } {
                            next!()
                        }
                        let key = konst!(inst.b());
                        save!();
                        self.newindex_miss(t, key, v)?;
                    }
                    Op::SetTable => set_arm!(regs.wrapping_add(inst.b() as usize)),
                    Op::SetField => set_arm!(kptr.wrapping_add(inst.b() as usize)),
                    Op::SetI => {
                        let t = reg!(inst.a());
                        let key = Value::Int(inst.b() as i64);
                        let v = reg!(inst.c());
                        if self.newindex_raw(t, key, v) {
                            next!()
                        }
                        save!();
                        self.newindex_miss(t, key, v)?;
                    }
                    Op::SelfOp => {
                        let pb = regs.wrapping_add(inst.b() as usize);
                        // SAFETY: a register of the running frame
                        let o = unsafe { *pb };
                        set_reg!(inst.a() + 1, o);
                        // PUC OP_SELF's C is a constant index when the k-flag is
                        // set; otherwise it points to a register that holds the
                        // (constant-loaded) key. luna's compiler falls back to the
                        // register form when the constant index exceeds OP_SELF's
                        // 8-bit C field (5.1 big.lua's `a:findfield(...)` against
                        // a table with 250+ string keys, where "findfield" lands
                        // past const #255). The exec must honour the same split.
                        let pk = if inst.k() {
                            kptr.wrapping_add(inst.c() as usize)
                        } else {
                            regs.wrapping_add(inst.c() as usize)
                        };
                        // SAFETY: a register or constant of the running frame;
                        // the object is read from its copy, `R[A]` may be `R[C]`
                        if let Some(v) = unsafe { Vm::index_raw_key_at(o, pk) } {
                            set_reg!(inst.a(), v);
                            next!()
                        }
                        // SAFETY: as above
                        let key = unsafe { *pk };
                        save!();
                        self.index_miss(o, key, base!() + inst.a())?;
                    }
                    Op::Add => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a.wrapping_add(b))), float(a, b) => Some(Value::Float(a + b)),
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Add, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::Sub => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a.wrapping_sub(b))), float(a, b) => Some(Value::Float(a - b)),
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Sub, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::Mul => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a.wrapping_mul(b))), float(a, b) => Some(Value::Float(a * b)),
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Mul, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    // a zero divisor takes the slow path for its error
                    Op::Mod => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => (b != 0).then(|| Value::Int(int_mod(a, b))), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Mod, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::IDiv => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::IDiv, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::Div => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Float(a as f64 / b as f64)), float(a, b) => Some(Value::Float(a / b)),
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Div, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::BAnd => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a & b)), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::BAnd, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::BOr => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a | b)), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::BOr, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::BXor => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(a ^ b)), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::BXor, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::Shl => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(shift_left(a, b))), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Shl, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::Shr => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), regs.wrapping_add(inst.c() as usize),
                            int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))), float(a, b) => { let _ = (a, b); None },
                            slow(l, r) => { save!(); self.arith_slow(inst.a(), base!(), ArithOp::Shr, l, r, inst.k()) })
                        {
                            next!()
                        }
                    }
                    Op::AddI => {
                        if arith_imm_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), inst.sc() as i64,
                            int(a, b) => Some(Value::Int(a.wrapping_add(b))), float(a, b) => Some(Value::Float(a + b)),
                            slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Add, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::SubI => {
                        if arith_imm_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), inst.sc() as i64,
                            int(a, b) => Some(Value::Int(a.wrapping_sub(b))), float(a, b) => Some(Value::Float(a - b)),
                            slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Sub, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::AddK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a.wrapping_add(b))), float(a, b) => Some(Value::Float(a + b)),
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Add, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::SubK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a.wrapping_sub(b))), float(a, b) => Some(Value::Float(a - b)),
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Sub, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::MulK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a.wrapping_mul(b))), float(a, b) => Some(Value::Float(a * b)),
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Mul, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    // a zero divisor takes the slow path for its error
                    Op::ModK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => (b != 0).then(|| Value::Int(int_mod(a, b))), float(a, b) => { let _ = (a, b); None },
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Mod, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::IDivK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => (b != 0).then(|| Value::Int(int_idiv(a, b))), float(a, b) => { let _ = (a, b); None },
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::IDiv, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::DivK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Float(a as f64 / b as f64)), float(a, b) => Some(Value::Float(a / b)),
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Div, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::PowK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Float(num_pow(self.version() >= LuaVersion::Lua54, a as f64, b as f64))), float(a, b) => Some(Value::Float(num_pow(self.version() >= LuaVersion::Lua54, a, b))),
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Pow, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::BAndK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a & b)), float(a, b) => { let _ = (a, b); None },
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::BAnd, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::BOrK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a | b)), float(a, b) => { let _ = (a, b); None },
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::BOr, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::BXorK => {
                        if arith_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), kptr.wrapping_add(inst.c() as usize),
                        int(a, b) => Some(Value::Int(a ^ b)), float(a, b) => { let _ = (a, b); None },
                        slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::BXor, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::ShrI => {
                        if arith_imm_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), inst.sc() as i64,
                            int(a, b) => Some(Value::Int(shift_left(a, b.wrapping_neg()))), float(a, b) => { let _ = (a, b); None },
                            slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Shr, l, r, false)
                        }) {
                            next!()
                        }
                    }
                    Op::ShlI => {
                        if arith_imm_arm!(regs, inst, regs.wrapping_add(inst.b() as usize), inst.sc() as i64,
                            int(a, b) => Some(Value::Int(shift_left(a, b))), float(a, b) => { let _ = (a, b); None },
                            slow(x, c) => {
                            save!();
                            let (l, r) = if inst.k() { (c, x) } else { (x, c) };
                            self.arith_slow(inst.a(), base!(), ArithOp::Shl, l, r, false)
                        }) {
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
                                save!();
                                let mm = self.get_mm(v, Mm::Unm);
                                if mm.is_nil() {
                                    return Err(self.type_err("perform arithmetic on", v));
                                }
                                let dst = base!() + inst.a();
                                self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                            }
                        }
                    }
                    Op::BNot => {
                        let v = reg!(inst.b());
                        match self.arith_operand()(v) {
                            Some(n) => {
                                let Some(i) = int_of(n) else {
                                    save!();
                                    return Err(self.no_int_rep_err());
                                };
                                set_reg!(inst.a(), Value::Int(!i));
                                next!()
                            }
                            None => {
                                save!();
                                let mm = self.get_mm(v, Mm::BNot);
                                if mm.is_nil() {
                                    return Err(self.type_err("perform bitwise operation on", v));
                                }
                                let dst = base!() + inst.a();
                                self.begin_meta_call(mm, &[v, v], MetaAction::Store { dst })?;
                            }
                        }
                    }
                    Op::Not => {
                        // SAFETY: a register of the running frame
                        let t = unsafe { raw_truthy(regs.add(inst.b() as usize)) };
                        set_reg!(inst.a(), Value::Bool(!t));
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
                        save!();
                        match self.len_step(v)? {
                            MmOut::Done(r) => self.set_r(base!(), inst.a(), r),
                            MmOut::Mm { func, recv } => {
                                let dst = base!() + inst.a();
                                self.begin_meta_call(
                                    func,
                                    &[recv, recv],
                                    MetaAction::Store { dst },
                                )?;
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
                            let proto = cl!().proto;
                            let c = proto.trace_hot_count.get();
                            if c < u32::MAX / 2 {
                                proto.trace_hot_count.set(c + 1);
                            }
                            let target = (pc as i32 + 1 + off).max(0) as u32;
                            save!();
                            if c >= self.jit.trace_hot_threshold
                                && self.trace_start_at_jmp(cl!(), base!(), target)
                            {
                                // the recording sees the next instruction from
                                // the loop head
                                // SAFETY: see `next!`
                                unsafe { (*fr).pc = npc };
                                return Ok(FastExit::Reload);
                            }
                        }
                        next!()
                    }
                    Op::Eq => {
                        let (pl, pr) = (
                            regs.wrapping_add(inst.a() as usize),
                            regs.wrapping_add(inst.b() as usize),
                        );
                        // SAFETY: registers of the running frame
                        let (tl, tr) = unsafe { (raw_tag(pl), raw_tag(pr)) };
                        // `__eq` is looked for only between two tables or two
                        // full userdata
                        let eq = if tl == tag::INT && tr == tag::INT {
                            // SAFETY: two integers
                            unsafe { raw_int(pl) == raw_int(pr) }
                        } else if tl != tr || tl != tag::TABLE && tl != tag::USERDATA {
                            // SAFETY: as above
                            unsafe { (*pl).raw_eq(*pr) }
                        } else {
                            // SAFETY: as above
                            let (l, r) = unsafe { (*pl, *pr) };
                            save!();
                            let step = self.eq_step(l, r);
                            self.op_compare(step, l, r, inst.k())?;
                            resume!()
                        };
                        if eq != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    // a constant is never a table or a userdata: no `__eq`
                    Op::EqK => {
                        let (pl, pk) = (
                            regs.wrapping_add(inst.a() as usize),
                            kptr.wrapping_add(inst.b() as usize),
                        );
                        // SAFETY: a register and a constant of the running frame
                        let eq = unsafe {
                            if raw_tag(pl) == tag::INT && raw_tag(pk) == tag::INT {
                                raw_int(pl) == raw_int(pk)
                            } else {
                                (*pl).raw_eq(*pk)
                            }
                        };
                        if eq != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    Op::Lt => order_arm!(<, false),
                    Op::Le => order_arm!(<=, true),
                    // raw equality with a number: no metamethod can be involved
                    Op::EqI => {
                        let px = regs.wrapping_add(inst.a() as usize);
                        let im = inst.sb();
                        // SAFETY: a register of the running frame
                        let eq = unsafe {
                            match raw_tag(px) {
                                tag::INT => raw_int(px) == im as i64,
                                tag::FLOAT => raw_flt(px) == im as f64,
                                _ => false,
                            }
                        };
                        if eq != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    Op::LtI => order_imm_arm!(<, false, false),
                    Op::LeI => order_imm_arm!(<=, false, true),
                    Op::GtI => order_imm_arm!(>, true, false),
                    Op::GeI => order_imm_arm!(>=, true, true),
                    Op::Test => {
                        // the JMP that follows runs when the condition equals k
                        // SAFETY: a register of the running frame
                        if unsafe { raw_truthy(regs.add(inst.a() as usize)) } != inst.k() {
                            npc += 1;
                        }
                        next!()
                    }
                    Op::TestSet => {
                        let pb = regs.wrapping_add(inst.b() as usize);
                        // SAFETY: a register of the running frame
                        if unsafe { raw_truthy(pb) } == inst.k() {
                            // SAFETY: as above
                            let v = unsafe { *pb };
                            set_reg!(inst.a(), v);
                        } else {
                            npc += 1;
                        }
                        next!()
                    }
                    Op::ForLoop => {
                        let ra = regs.wrapping_add(inst.a() as usize);
                        let back = npc.wrapping_sub(inst.bx());
                        let mut slow = false;
                        // SAFETY: the loop's four registers are in the frame
                        // (the verifier checks the run)
                        let (t0, t1, t2) =
                            unsafe { (raw_tag(ra), raw_tag(ra.add(1)), raw_tag(ra.add(2))) };
                        if t0 == tag::INT && t1 == tag::INT && t2 == tag::INT {
                            // SAFETY: three integers; the index and the count
                            // or limit keep their tags, the control variable
                            // is the body's to change
                            unsafe {
                                let (cur, x, st) =
                                    (raw_int(ra), raw_int(ra.add(1)), raw_int(ra.add(2)));
                                if !pre53 {
                                    if x != 0 {
                                        let next = cur.wrapping_add(st);
                                        put_int(ra, next);
                                        put_int(ra.add(1), x.wrapping_sub(1));
                                        ra.add(3).write(Value::Int(next));
                                        npc = back;
                                    }
                                } else {
                                    let next = cur.wrapping_add(st);
                                    if if st > 0 { next <= x } else { next >= x } {
                                        put_int(ra, next);
                                        ra.add(3).write(Value::Int(next));
                                        npc = back;
                                    }
                                }
                            }
                        } else if t0 == tag::FLOAT && t1 == tag::FLOAT && t2 == tag::FLOAT {
                            // SAFETY: three floats
                            unsafe {
                                let (cur, lim, st) =
                                    (raw_flt(ra), raw_flt(ra.add(1)), raw_flt(ra.add(2)));
                                let next = cur + st;
                                if if st > 0.0 { next <= lim } else { next >= lim } {
                                    ra.write(Value::Float(next));
                                    ra.add(3).write(Value::Float(next));
                                    npc = back;
                                }
                            }
                        } else {
                            // `for_loop` is the reference: 5.1–5.3 step and
                            // compare with the limit, 5.4+ count down; anything
                            // else it raises on
                            save!();
                            self.for_loop(inst, base!())?;
                            npc = self.top_frame().pc;
                            slow = true;
                        }
                        // The trace JIT counts the back-edges taken and starts
                        // recording at the body once the count reaches the
                        // threshold.
                        if trace_on && npc != pc + 1 {
                            let proto = cl!().proto;
                            let c = proto.trace_hot_count.get();
                            if c < u32::MAX / 2 {
                                proto.trace_hot_count.set(c + 1);
                            }
                            if c == self.jit.trace_hot_threshold && self.jit.active_trace.is_none()
                            {
                                // the back-edge target is the body's first op
                                let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                                save!();
                                self.trace_start_at_loop(cl!(), base!(), target, None);
                                slow = true;
                            }
                        }
                        if slow {
                            // SAFETY: see `next!`
                            unsafe { (*fr).pc = npc };
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
                                let proto = cl!().proto;
                                let c = proto.trace_hot_count.get();
                                if c < u32::MAX / 2 {
                                    proto.trace_hot_count.set(c + 1);
                                }
                                if c == self.jit.trace_hot_threshold
                                    && self.jit.active_trace.is_none()
                                {
                                    // the body's first op, right after TForPrep
                                    let target = (pc as i32 + 1 - inst.bx() as i32).max(0) as u32;
                                    save!();
                                    self.trace_start_at_loop(cl!(), base!(), target, Some(a));
                                }
                            }
                            set_reg!(a + 2, ctrl);
                            npc = npc.wrapping_sub(inst.bx());
                            // a recording that just started must see the next
                            // instruction from the loop head
                            if trace_on && self.jit.active_trace.is_some() {
                                // SAFETY: see `next!`
                                unsafe { (*fr).pc = npc };
                                return Ok(FastExit::Reload);
                            }
                        }
                        next!()
                    }
                    Op::VargIdx => {
                        // R[A] := vararg[R[C]] without allocating: integer key in
                        // [1,n] → that vararg, "n" → the count, else nil.
                        let key = reg!(inst.c());
                        // SAFETY: `fr` is the running frame
                        let (fs, n) = unsafe { ((*fr).func_slot, (*fr).n_varargs) };
                        let v = match key {
                            Value::Int(k) if k >= 1 && (k as u64) <= n as u64 => {
                                self.stack[(fs + k as u32) as usize]
                            }
                            Value::Float(f) if f.fract() == 0.0 && f >= 1.0 && f <= n as f64 => {
                                self.stack[(fs + f as u32) as usize]
                            }
                            Value::Str(s) if s.as_bytes() == b"n" => Value::Int(n as i64),
                            _ => Value::Nil,
                        };
                        set_reg!(inst.a(), v);
                        next!()
                    }
                    Op::ErrNNil => {
                        let v = self.r(base!(), inst.a());
                        if !matches!(v, Value::Nil) {
                            let bx = inst.bx();
                            let name = if bx == 0 {
                                "?".to_string()
                            } else {
                                match cl!().proto.consts[(bx - 1) as usize] {
                                    Value::Str(s) => {
                                        String::from_utf8_lossy(s.as_bytes()).into_owned()
                                    }
                                    _ => "?".to_string(),
                                }
                            };
                            save!();
                            return Err(self.rt_err(&format!("global '{name}' already defined")));
                        }
                        next!()
                    }
                    Op::Call => {
                        save!();
                        let abs = base!() + inst.a();
                        let nargs = if inst.b() == 0 {
                            None
                        } else {
                            Some(inst.b() - 1)
                        };
                        let wanted = inst.c() as i32 - 1;
                        let pf = regs.wrapping_add(inst.a() as usize);
                        if !WATCH {
                            // SAFETY: the called register is in the frame
                            let t = unsafe { raw_tag(pf) };
                            if t == tag::CLOSURE && !trace_on {
                                // SAFETY: a closure tag means a live closure
                                let callee = Gc::from_ptr(unsafe { raw_gc(pf) } as *mut LuaClosure);
                                let n = nargs.unwrap_or_else(|| self.top - (abs + 1));
                                if self.push_lua_frame_fast(callee, abs, n, wanted) {
                                    continue 'frames;
                                }
                            } else if t == tag::NATIVE {
                                // SAFETY: a native tag means a live native closure
                                let nc = Gc::from_ptr(
                                    unsafe { raw_gc(pf) } as *mut crate::runtime::NativeClosure
                                );
                                if nc.kind == NativeKind::Plain {
                                    let n = nargs.unwrap_or_else(|| self.top - (abs + 1));
                                    self.call_native_plain(nc, abs, n, wanted)?;
                                    resume!()
                                }
                            }
                        }
                        self.begin_call(abs, nargs, wanted, false)?;
                        reenter!()
                    }
                    // the common returns: to a Lua caller or a metamethod's
                    // continuation in this activation, with nothing to close and
                    // no hook (see `return_fast`); the loop head's `Return` arm
                    // does the rest
                    Op::Return0 => {
                        let base = base!();
                        if self.return_fast(base, base, 0, entry_depth) {
                            reenter!()
                        }
                        save!();
                        return Ok(FastExit::Slow(inst));
                    }
                    Op::Return1 => {
                        let base = base!();
                        if self.return_fast(base, base + inst.a(), 1, entry_depth) {
                            reenter!()
                        }
                        save!();
                        return Ok(FastExit::Slow(inst));
                    }
                    // they stay in this frame: run out of line, then go on
                    Op::LoadKx
                    | Op::NewTable
                    | Op::SetList
                    | Op::Pow
                    | Op::Concat
                    | Op::ForPrep
                    | Op::TForPrep
                    | Op::Closure
                    | Op::Vararg
                    | Op::GetVarg => {
                        save!();
                        self.run_frame_op(inst)?
                    }
                    // listed rather than `_`, so that the jump table covers every
                    // opcode without a range check
                    Op::Close
                    | Op::Tbc
                    | Op::TailCall
                    | Op::Return
                    | Op::TForCall
                    | Op::ExtraArg => {
                        save!();
                        return Ok(FastExit::Slow(inst));
                    }
                }
                // a fast arm's slow path
                resume!()
            }
        }
    }

    /// `trap` is set after a call or return: when that is only because a
    /// metamethod call pushed its continuation, or a metamethod returned to
    /// one, finish what the loop head would and report whether a Lua frame
    /// with nothing to watch is now on top. Anything else (a hook, a budget,
    /// a memory cap, another kind of continuation) is left to the loop head.
    #[inline(never)]
    fn settle_frames(&mut self, entry_depth: usize) -> Result<bool, LuaError> {
        if self.instr_budget.is_some() || self.heap.mem_cap.is_some() || self.hook_armed() {
            return Ok(false);
        }
        loop {
            match self.frames.last() {
                Some(CallFrame::Lua(_)) => {
                    self.trap = false;
                    return Ok(true);
                }
                Some(&CallFrame::Cont(nc)) if matches!(nc.kind, ContKind::Meta(_)) => {
                    // a metamethod's result completes the instruction; this
                    // kind never hands results out of the activation
                    let out = self.finish_cont(nc, entry_depth)?;
                    debug_assert!(out.is_none());
                }
                _ => return Ok(false),
            }
        }
    }
}
