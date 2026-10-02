use super::*;

pub(super) fn emit_basic(
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let EmitFacts {
        c: ChunkIn {
            proto, float_only, ..
        },
        reg_kinds,
        regs,
        pc_to_block,
        ..
    } = f;
    let EmitState {
        current_kinds,
        current_is_nil,
        terminated,
        ..
    } = st;
    match ins.op() {
        Op::LoadI => {
            let imm = ins.sbx() as i64;
            let v = bcx.ins().iconst(types::I64, imm);
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, v);
            current_kinds[ins.a() as usize] = RegKind::Int;
            current_is_nil[ins.a() as usize] = false;
        }
        Op::LoadF => {
            let f = ins.sbx() as f64;
            let v = bcx.ins().f64const(f);
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, v);
            current_kinds[ins.a() as usize] = RegKind::Float;
            current_is_nil[ins.a() as usize] = false;
        }
        Op::LoadK => {
            // Whitelist ensures Int or Float const.
            let bx = ins.bx() as usize;
            let (v, k) = match proto.consts[bx] {
                LuaValue::Float(f) => (bcx.ins().f64const(f), RegKind::Float),
                LuaValue::Int(i) => (bcx.ins().iconst(types::I64, i), RegKind::Int),
                _ => unreachable!("scanner rejects non-numeric LoadK"),
            };
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, v);
            current_kinds[ins.a() as usize] = k;
            current_is_nil[ins.a() as usize] = false;
        }
        Op::LoadNil => {
            // `R[A..=A+B] = nil`. Lower to a sequence of
            // `iconst(0)` writes, then flag `current_is_nil` so the
            // matching SetList in this BB picks `RAW_TAG_NIL` over
            // the default Int tag. The `aligned_def` accepts any
            // declared kind because the 8-byte payload of Nil is 0
            // (lossless bitcast to F64 or I64).
            let zero = bcx.ins().iconst(types::I64, 0);
            let a = ins.a() as usize;
            for off in 0..=(ins.b() as usize) {
                let r = a + off;
                aligned_def(bcx, regs, reg_kinds, r, zero);
                current_is_nil[r] = true;
            }
        }
        Op::Move => {
            let src = bcx.use_var(regs[ins.b() as usize]);
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, src);
            current_kinds[ins.a() as usize] = current_kinds[ins.b() as usize];
            current_is_nil[ins.a() as usize] = current_is_nil[ins.b() as usize];
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            let lhs = bcx.use_var(regs[ins.b() as usize]);
            let rhs = bcx.use_var(regs[ins.c() as usize]);
            // Destination kind picked from the sweep's final
            // `reg_kinds` — `current_kinds[a]` reflects pre-write
            // state and may still be Unset before this op runs.
            let k = a_kind(reg_kinds, ins.a());
            // a register a table was stored in first keeps the Table
            // kind when a later arithmetic result lands there; its
            // result tag would be wrong, so leave the function to the
            // interpreter
            if k == RegKind::Table {
                return None;
            }
            // 5.1/5.2 integers stand for doubles, whose sums round and
            // whose products can be -0: the interpreter's to compute
            if float_only && k != RegKind::Float {
                return None;
            }
            // A float result converts an integer operand first
            // (`a / b` of two integers, or `i + 0.5`).
            let (lhs, rhs) = if k == RegKind::Float {
                let to_float = |bcx: &mut FunctionBuilder<'_>, v: Value| {
                    if bcx.func.dfg.value_type(v) == types::I64 {
                        bcx.ins().fcvt_from_sint(types::F64, v)
                    } else {
                        v
                    }
                };
                (to_float(bcx, lhs), to_float(bcx, rhs))
            } else {
                (lhs, rhs)
            };
            let r = match (ins.op(), k) {
                (Op::Add, RegKind::Float) => bcx.ins().fadd(lhs, rhs),
                (Op::Sub, RegKind::Float) => bcx.ins().fsub(lhs, rhs),
                (Op::Mul, RegKind::Float) => bcx.ins().fmul(lhs, rhs),
                (Op::Div, RegKind::Float) => bcx.ins().fdiv(lhs, rhs),
                (Op::Add, _) => bcx.ins().iadd(lhs, rhs),
                (Op::Sub, _) => bcx.ins().isub(lhs, rhs),
                (Op::Mul, _) => bcx.ins().imul(lhs, rhs),
                (Op::Div, _) => unreachable!("Op::Div scan pins result to Float"),
                _ => unreachable!(),
            };
            aligned_def(bcx, regs, reg_kinds, ins.a() as usize, r);
            current_kinds[ins.a() as usize] = k;
            current_is_nil[ins.a() as usize] = false;
        }
        Op::Return1 => {
            let v = bcx.use_var(regs[ins.a() as usize]);
            let out = if matches!(a_kind(reg_kinds, ins.a()), RegKind::Float) {
                bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
            } else {
                v
            };
            bcx.ins().return_(&[out]);
            *terminated = true;
        }
        Op::Return0 => {
            let zero = bcx.ins().iconst(types::I64, 0);
            bcx.ins().return_(&[zero]);
            *terminated = true;
        }
        Op::Jmp => {
            let tgt = jmp_target(pc, ins);
            let tgt_blk = pc_to_block[tgt].expect("Jmp target is BB start");
            bcx.ins().jump(tgt_blk, &[]);
            *terminated = true;
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

/// Returns the pc emit continues from: the comparison consumes its
/// paired `Jmp`.
pub(super) fn emit_cmp(
    bcx: &mut FunctionBuilder<'_>,
    st: &mut EmitState,
    f: EmitFacts<'_>,
    pc: usize,
    ins: Inst,
) -> usize {
    let EmitFacts {
        c: ChunkIn { code, .. },
        reg_kinds,
        regs,
        pc_to_block,
        ..
    } = f;
    let EmitState { terminated, .. } = st;
    let mut pc = pc;
    match ins.op() {
        Op::Lt | Op::Le | Op::Eq => {
            let jmp = code[pc + 1];
            debug_assert!(matches!(jmp.op(), Op::Jmp), "scanner enforces pairing");
            let lhs = bcx.use_var(regs[ins.a() as usize]);
            let rhs = bcx.use_var(regs[ins.b() as usize]);
            let lhs_kind = a_kind(reg_kinds, ins.a());
            let rhs_kind = a_kind(reg_kinds, ins.b());
            // Operand kinds were unified by the scan; if either is
            // Float they both are.
            let cond = if matches!(lhs_kind, RegKind::Float) || matches!(rhs_kind, RegKind::Float) {
                let fcc = match ins.op() {
                    Op::Lt => FloatCC::LessThan,
                    Op::Le => FloatCC::LessThanOrEqual,
                    Op::Eq => FloatCC::Equal,
                    _ => unreachable!(),
                };
                bcx.ins().fcmp(fcc, lhs, rhs)
            } else {
                let icc = match ins.op() {
                    Op::Lt => IntCC::SignedLessThan,
                    Op::Le => IntCC::SignedLessThanOrEqual,
                    Op::Eq => IntCC::Equal,
                    _ => unreachable!(),
                };
                bcx.ins().icmp(icc, lhs, rhs)
            };
            // PUC `cond_skip`: bump_pc (skip the Jmp) if cond != k;
            // otherwise execute the Jmp. So `cond == k` → take Jmp;
            // `cond != k` → fall through past Jmp.
            let fall_blk = pc_to_block[pc + 2].expect("fallthrough BB");
            let jmp_blk = pc_to_block[jmp_target(pc + 1, jmp)].expect("Jmp target BB");
            if ins.k() {
                // k=true: take jmp when cond=1; fall when cond=0.
                bcx.ins().brif(cond, jmp_blk, &[], fall_blk, &[]);
            } else {
                // k=false: take jmp when cond=0; fall when cond=1.
                bcx.ins().brif(cond, fall_blk, &[], jmp_blk, &[]);
            }
            *terminated = true;
            pc += 1; // consume the paired Jmp; outer increment moves past it
        }
        _ => unreachable!("dispatched by op"),
    }
    pc
}
