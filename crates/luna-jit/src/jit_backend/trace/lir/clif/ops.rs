//! The arithmetic, branches and calls of [`Replay`], and the relocations it
//! reads in the entry block.

use super::*;

impl Replay<'_, '_> {
    pub(super) fn bin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        let ins = self.b.ins();
        match op {
            BinOp::Add => ins.iadd(x, y),
            BinOp::Sub => ins.isub(x, y),
            BinOp::Mul => ins.imul(x, y),
            BinOp::Sdiv => ins.sdiv(x, y),
            BinOp::Udiv => ins.udiv(x, y),
            BinOp::Umulhi => ins.umulhi(x, y),
            BinOp::Smin => ins.smin(x, y),
            BinOp::Smax => ins.smax(x, y),
            BinOp::And => ins.band(x, y),
            BinOp::Or => ins.bor(x, y),
            BinOp::Xor => ins.bxor(x, y),
            BinOp::Shl => ins.ishl(x, y),
            BinOp::Ushr => ins.ushr(x, y),
            BinOp::Sshr => ins.sshr(x, y),
            BinOp::Fadd => ins.fadd(x, y),
            BinOp::Fsub => ins.fsub(x, y),
            BinOp::Fmul => ins.fmul(x, y),
            BinOp::Fdiv => ins.fdiv(x, y),
        }
    }

    pub(super) fn un(&mut self, u: UnOp, i: &Inst) -> Value {
        let x = self.v(i.a);
        let from = self.lir.value_ty[i.a as usize];
        let t = cty(i.ty);
        let ins = self.b.ins();
        match u {
            UnOp::Ineg => ins.ineg(x),
            UnOp::Bnot => ins.bnot(x),
            UnOp::Fneg => ins.fneg(x),
            UnOp::Floor => ins.floor(x),
            UnOp::Ceil => ins.ceil(x),
            UnOp::Uextend if from == i.ty => x,
            UnOp::Uextend => ins.uextend(t, x),
            UnOp::Ireduce if from == i.ty => x,
            UnOp::Ireduce => ins.ireduce(t, x),
            UnOp::Bitcast => ins.bitcast(t, MemFlagsData::new(), x),
            UnOp::FcvtFromSint => ins.fcvt_from_sint(t, x),
            UnOp::FcvtToSint => ins.fcvt_to_sint(t, x),
            UnOp::FcvtToSintSat => ins.fcvt_to_sint_sat(t, x),
        }
    }

    /// Replays a call, direct or through a pointer.
    fn call(&mut self, i: &Inst) {
        let lir = self.lir;
        let (callee, target) = match i.op {
            Op::Call => {
                let f = &lir.funcs[i.a as usize];
                (f, self.b.ins().iconst(types::I64, f.addr as i64))
            }
            _ => (&lir.sigs[i.b as usize], self.v(i.a)),
        };
        let sig = sig_of(lir, callee, self.call_conv);
        let sref = self.b.import_signature(sig);
        let a = self.args(i);
        let call = self.b.ins().call_indirect(sref, target, &a);
        if i.dst != NONE {
            let r = self.b.inst_results(call)[0];
            self.set(i.dst, r);
        }
    }

    /// Replays a jump: a `Pick`'s to the block the function takes, a calling
    /// exit's to the call block, or a return of the exit's return when the
    /// function's calling exits leave its cell to the dispatcher.
    fn goto(&mut self, i: &Inst) {
        let a = self.args(i);
        let to = match i.op {
            Op::Pick if self.start == live::Start::Loop => i.a,
            Op::Pick => i.b,
            Op::CallingExit(n) if !self.calls.has(n) => {
                self.b.ins().return_(&a[1..2]);
                return;
            }
            _ => i.a,
        };
        let target = self.blocks[to as usize].expect("reachable");
        let ba = self.block_args(&a);
        self.b.ins().jump(target, &ba);
    }

    /// Replays a branch, a call or a return.
    pub(super) fn control(&mut self, i: &Inst) {
        match i.op {
            Op::Jump | Op::Pick | Op::CallingExit(_) => self.goto(i),
            Op::Brif(n_then) => {
                let a = self.args(i);
                let (ta, ea) = a.split_at(n_then as usize);
                let (tb, eb) = (
                    self.blocks[i.b as usize].expect("reachable"),
                    self.blocks[i.c as usize].expect("reachable"),
                );
                let (ta, ea) = (self.block_args(ta), self.block_args(ea));
                let o0 = self.v(i.a);
                self.b.ins().brif(o0, tb, &ta, eb, &ea);
            }
            // only the baseline tier counts iterations; the first pass's
            // function goes on in the loop's (see `lower::handoff`)
            Op::TierCount { .. } => {
                let to = match self.start {
                    live::Start::FirstPass => i.b,
                    _ => i.c,
                };
                let to = self.blocks[to as usize].expect("reachable");
                self.b.ins().jump(to, &[]);
            }
            Op::Call | Op::CallIndirect => self.call(i),
            Op::Return => {
                let r: Vec<Value> = (i.a != NONE).then(|| self.v(i.a)).into_iter().collect();
                self.b.ins().return_(&r);
            }
            _ => unreachable!("not a branch, call or return"),
        }
    }
}

/// What reading a relocation where it is used costs, in instructions, on
/// `arch`: a `movabs` on x86-64; elsewhere (AArch64) a literal load and
/// the branch over the eight bytes it reads.
pub(super) fn read_in_place_cost(arch: target_lexicon::Architecture) -> u32 {
    match arch {
        target_lexicon::Architecture::X86_64 => 1,
        _ => 2,
    }
}

/// Reads the relocations [`Replay::hoisted`] keeps, at the start of the
/// entry block: string and function ones whose reads inside loops, each
/// costing `read_cost` instructions there, cost more than one reload of
/// a spilled register would, the most read first, at most [`HOISTED`].
/// So x86-64 keeps those read at least twice, and AArch64, where a read
/// in place is a load and a branch, every one a loop reads.
pub(super) fn hoist(
    r: &mut Replay<'_, '_>,
    an: &live::Analysis,
    relocs: &[(crate::jit_backend::trace::RelocKind, i64)],
    read_cost: u32,
) {
    use crate::jit_backend::trace::RelocKind;
    // a reload: one load
    const RELOAD: u32 = 1;
    let mut in_loops = vec![0u32; relocs.len()];
    for &(head, back) in &an.loops {
        for c in head / 2..=back / 2 {
            if let Op::Reloc(n) = r.lir.insts[an.code[c as usize] as usize].op {
                in_loops[n as usize] += 1;
            }
        }
    }
    let mut keep: Vec<usize> = (0..relocs.len())
        .filter(|&n| {
            in_loops[n] * read_cost > RELOAD
                && matches!(relocs[n].0, RelocKind::Str | RelocKind::Proto)
        })
        .collect();
    keep.sort_by_key(|&n| std::cmp::Reverse(in_loops[n]));
    for &n in keep.iter().take(HOISTED) {
        let var = r.b.declare_var(types::I64);
        r.hoisted[n] = Some(var);
    }
    rehoist(r);
}

/// Reads the relocations [`Replay::hoisted`] keeps into their variables.
pub(super) fn rehoist(r: &mut Replay<'_, '_>) {
    for (n, var) in r.hoisted.iter().enumerate() {
        if let Some(var) = *var {
            let v = r.b.ins().symbol_value(types::I64, r.reloc_gv[n]);
            r.b.def_var(var, v);
        }
    }
}
