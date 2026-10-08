use super::kinds_for::sweep_for;
use super::*;

pub(super) struct KindSweep {
    pub(super) reg_kinds: Vec<RegKind>,
    pub(super) ret_kind: RegKind,
    // `latest_writer_kind[reg]` records the kind written to `reg`
    // by the most recent writer op in linear PC order during this
    // sweep pass. With the current per-proto `RegKind` model
    // (strict Int/Table conflict), this tracker is a no-op: every
    // op that writes a Variable's kind also passes through the
    // global unify, so latest_writer_kind never disagrees with
    // reg_kinds. The scaffold is wired in so a relaxed
    // `unify` (e.g. Int + Table → joint) lets the Return1 ret
    // kind can be picked from the latest writer rather than the
    // joint kind. See `make_proto_5_5_round_trip` (currently still
    // bails) for the motivating shape.
    pub(super) latest_writer_kind: Vec<RegKind>,
    // `maybe_table[reg]` is set when the register
    // could hold a Table pointer at runtime even though
    // `reg_kinds[reg]` says Int. The classic case is
    // `Op::GetI R[A] = R[B][c]`: the helper returns the raw
    // payload bits regardless of the stored Value's tag, so
    // when the slot held a Table at runtime R[A] is a Gc<Table>
    // pun. Downstream arith / Lt-Le / ForPrep use this tag to
    // bail conservatively (interp would have raised; the JIT'd
    // `iadd` / `icmp` would silently compute garbage).
    pub(super) maybe_table: Vec<bool>,
    // parallel to `maybe_table`: this register's most
    // recent writer was `Op::LoadNil`, so a kind-sensitive reader
    // (arith, cmp, SetTable's helper) would silently read `Int(0)`
    // where the Lua semantics demand a Nil error or Nil tag. SetList
    // emit consumes Nil-tagged stores via `current_is_nil` and so
    // does NOT bail on a Nil source; arith/cmp/SetTable scan bail.
    pub(super) is_nil_writer: Vec<bool>,
}

/// Returns the registers' kinds and the return kind.
pub(super) fn sweep_kinds(
    c: ChunkIn<'_>,
    scan: &ChunkScan,
    cfg: &ChunkCfg,
) -> Option<(Vec<RegKind>, RegKind)> {
    let ChunkIn {
        code,
        n,
        num_params,
        max_stack,
        ..
    } = c;
    // per-register type inference. Each Lua register holds either
    // an Int (i64) or a Float (f64). A register that's pinned to both
    // shapes within the same Proto bails the lowerer. The sweep is
    // forward-only with a fixpoint loop because a self-recursive Call
    // result kind depends on the Proto's own return kind (carried via
    // `ret_kind`); successive passes propagate the resolved kind.
    let may_nil = nil_flow::may_nil_by_pc(c, cfg);
    let mut st = KindSweep {
        reg_kinds: vec![RegKind::Unset; max_stack],
        ret_kind: RegKind::Unset,
        latest_writer_kind: Vec::new(),
        maybe_table: Vec::new(),
        is_nil_writer: Vec::new(),
    };
    for _ in 0..4 {
        let pre_regs = st.reg_kinds.clone();
        let pre_ret = st.ret_kind;
        st.latest_writer_kind = vec![RegKind::Unset; max_stack];
        st.maybe_table = vec![false; max_stack];
        // Function args (R[0..num_params]) carry valid Values from the
        // caller. Locals (R[num_params..max_stack]) come in as Nil from
        // the interp's frame-init clear. PUC 5.1 optimizes away the
        // LoadNil for declared-uninitialized locals at function start
        // (`luaK_nil` suppresses if pc==0 + reg above nactvar). Without
        // pre-marking those as nil_writer, JIT arith reading an
        // uninitialized 5.1 local would silently consume the cranelift
        // Variable's default 0 instead of raising "arithmetic on nil".
        // See docs/known-bugs/fixed/jit-uninitialized-local-arith.md
        // (filed 2026-06-22 by luna-core/tests/it/e2e_programs.rs::err_arith_on_nil).
        st.is_nil_writer = vec![false; max_stack];
        for r in num_params..max_stack {
            st.is_nil_writer[r] = true;
        }
        let mut pc = 0;
        while pc < n {
            let ins = code[pc];
            // the walk is in pc order, so a write it passed (one branch's)
            // has not happened on every path: take the per-path state where
            // some path reaches the op
            if let Some(m) = &may_nil[pc] {
                st.is_nil_writer[..m.len()].copy_from_slice(m);
            }
            match ins.op() {
                Op::LoadI if scan.dead_loads[pc] => {}
                Op::LoadI | Op::LoadF | Op::LoadK | Op::LoadNil | Op::Move => {
                    sweep_loads(&mut st, c, scan, pc, ins)?
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Lt | Op::Le | Op::Eq => {
                    sweep_arith_cmp(&mut st, ins)?
                }
                Op::GetUpval
                | Op::Call
                | Op::GetTabUp
                | Op::GetField
                | Op::Return1
                | Op::Return0
                | Op::Jmp => kinds_ops::sweep_calls(&mut st, scan, pc, ins)?,
                Op::ForPrep | Op::ForLoop | Op::ForPrep55 | Op::ForLoop55 => {
                    sweep_for(&mut st, ins)?
                }
                Op::NewTable | Op::SetList | Op::SetTable => {
                    kinds_ops::sweep_table_sets(&mut st, ins)?
                }
                Op::GetI | Op::GetTable | Op::Len => kinds_ops::sweep_table_gets(&mut st, c, ins)?,
                _ => return None,
            }
            pc += 1;
        }
        if st.reg_kinds == pre_regs && st.ret_kind == pre_ret {
            break;
        }
    }
    Some((st.reg_kinds, st.ret_kind))
}

fn sweep_loads(
    st: &mut KindSweep,
    c: ChunkIn<'_>,
    scan: &ChunkScan,
    pc: usize,
    ins: Inst,
) -> Option<()> {
    let ChunkIn { proto, .. } = c;
    let folded_math = &scan.folded_math;
    let KindSweep {
        reg_kinds,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
        ..
    } = st;
    match ins.op() {
        Op::LoadI => {
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Int) {
                return None;
            }
            latest_writer_kind[ins.a() as usize] = RegKind::Int;
            maybe_table[ins.a() as usize] = false;
            is_nil_writer[ins.a() as usize] = false;
        }
        Op::LoadF => {
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Float) {
                return None;
            }
            latest_writer_kind[ins.a() as usize] = RegKind::Float;
            maybe_table[ins.a() as usize] = false;
            is_nil_writer[ins.a() as usize] = false;
        }
        Op::LoadK => {
            // Whitelist guarantees Int or Float const.
            let bx = ins.bx() as usize;
            let kind = match proto.consts[bx] {
                LuaValue::Float(_) => RegKind::Float,
                LuaValue::Int(_) => RegKind::Int,
                _ => unreachable!("whitelist gates non-numeric consts"),
            };
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], kind) {
                return None;
            }
            latest_writer_kind[ins.a() as usize] = kind;
            maybe_table[ins.a() as usize] = false;
            is_nil_writer[ins.a() as usize] = false;
        }
        Op::LoadNil => {
            // `R[A..=A+B] = nil`. Leave `reg_kinds`
            // alone so a downstream writer (e.g. 5.1/5.2's
            // `LoadF R[3] = 1.0` after an earlier
            // `LoadNil R[3]` in the same Proto) can pin its
            // own kind without a unify conflict. The 8-byte
            // payload of Nil is 0, which is a lossless bit
            // pattern under either I64 or F64 Variable
            // (`aligned_def` bitcasts at the write site).
            // SetList emit overrides to `RAW_TAG_NIL` via the
            // BB-local `current_is_nil` shadow. The
            // `is_nil_writer` sweep tracker propagates through
            // Move and bails any arith/cmp/SetTable/Return1
            // reader so e.g. `nil + 1` or `t[nil] = 1` or
            // `return nil` falls through to the interpreter
            // (which raises the correct error or returns Nil).
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            for off in 0..=b {
                let r = a + off;
                // `latest_writer_kind` left untouched: a
                // subsequent reader that bypassed the
                // `is_nil_writer` bail would land on the prior
                // writer's kind, which is correct.
                maybe_table[r] = false;
                is_nil_writer[r] = true;
            }
        }
        Op::Move => {
            // fold-internal Move (slot +2 of a math
            // libcall) writes a temp register the libm emit
            // never reads (the emit pulls the arg straight
            // from `fold.arg_reg`). The temp gets clobbered
            // by the next opcode — either by the same fold's
            // Call result, or by a subsequent fold's
            // GetTabUp. Forcing a kind here just creates a
            // false conflict.
            if folded_math[pc] {
                // pc advances normally at the loop's tail.
            } else {
                let src_kind = reg_kinds[ins.b() as usize];
                if !RegKind::unify(&mut reg_kinds[ins.a() as usize], src_kind) {
                    return None;
                }
                // a source with no kind of its own (a parameter only
                // copied, as in `local f = p`) takes the destination's:
                // the copy reads its bits as that kind, so the entry
                // check must hold the argument to it
                if src_kind == RegKind::Unset {
                    reg_kinds[ins.b() as usize] = reg_kinds[ins.a() as usize];
                }
                let lwk = latest_writer_kind[ins.b() as usize];
                latest_writer_kind[ins.a() as usize] = lwk;
                maybe_table[ins.a() as usize] = maybe_table[ins.b() as usize];
                is_nil_writer[ins.a() as usize] = is_nil_writer[ins.b() as usize];
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}

fn sweep_arith_cmp(st: &mut KindSweep, ins: Inst) -> Option<()> {
    let KindSweep {
        reg_kinds,
        latest_writer_kind,
        maybe_table,
        is_nil_writer,
        ..
    } = st;
    match ins.op() {
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            // Table operand makes Lua's interp
            // error ("attempt to perform arithmetic on a
            // table value") while the JIT's `iadd` would
            // happily compute on ptr bits. Check
            // `reg_kinds`, `latest_writer_kind`,
            // and `maybe_table` (GetI returns whose
            // payload could be a stored Table).
            if matches!(reg_kinds[b], RegKind::Table)
                || matches!(reg_kinds[c], RegKind::Table)
                || matches!(latest_writer_kind[b], RegKind::Table)
                || matches!(latest_writer_kind[c], RegKind::Table)
                || maybe_table[b]
                || maybe_table[c]
            {
                return None;
            }
            // `nil + x` / `x + nil` raises in interp
            // (`attempt to perform arithmetic on a nil value`);
            // the JIT would silently `iadd(0, x)`. Bail so the
            // interpreter surfaces the error.
            if is_nil_writer[b] || is_nil_writer[c] {
                return None;
            }
            let kb = reg_kinds[b];
            let kc = reg_kinds[c];
            if !RegKind::unify(&mut reg_kinds[b], kc) {
                return None;
            }
            if !RegKind::unify(&mut reg_kinds[c], kb) {
                return None;
            }
            let merged = reg_kinds[b];
            if !RegKind::unify(&mut reg_kinds[ins.a() as usize], merged) {
                return None;
            }
            // Op::Div is Float-only in PUC 5.5 semantics
            // (integer `/` always coerces to float). Pin to
            // Float here so a chunk like `local x = a / b`
            // where a/b are Unset still resolves.
            if matches!(ins.op(), Op::Div)
                && !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Float)
            {
                return None;
            }
            // Arith result is Int or Float — never Table —
            // so clear the maybe_table tag on R[A].
            maybe_table[ins.a() as usize] = false;
            is_nil_writer[ins.a() as usize] = false;
            // Arith result kind picked from the operands'
            // local kinds: any Float → Float (PUC's mixed
            // promotion semantic); else Int.
            let lwk_b = latest_writer_kind[b];
            let lwk_c = latest_writer_kind[c];
            let arith_kind = if matches!(lwk_b, RegKind::Float)
                || matches!(lwk_c, RegKind::Float)
                || matches!(ins.op(), Op::Div)
            {
                RegKind::Float
            } else {
                RegKind::Int
            };
            latest_writer_kind[ins.a() as usize] = arith_kind;
        }
        Op::Lt | Op::Le | Op::Eq => {
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            // Lt/Le errors on a Table; Eq is
            // semantically safe (Lua's Eq across types is
            // always false, and our icmp on ptr bits
            // matches that for typical addresses).
            if matches!(ins.op(), Op::Lt | Op::Le)
                && (matches!(reg_kinds[a], RegKind::Table)
                    || matches!(reg_kinds[b], RegKind::Table)
                    || matches!(latest_writer_kind[a], RegKind::Table)
                    || matches!(latest_writer_kind[b], RegKind::Table)
                    || maybe_table[a]
                    || maybe_table[b])
            {
                return None;
            }
            // `nil < x` / `nil <= x` raise; `nil == x`
            // is well-defined in Lua but our icmp would compare
            // raw 0 bits ≠ proper Nil tag and miss the nil-aware
            // path. Bail conservatively.
            if is_nil_writer[a] || is_nil_writer[b] {
                return None;
            }
            let ka = reg_kinds[a];
            let kb = reg_kinds[b];
            if !RegKind::unify(&mut reg_kinds[a], kb) {
                return None;
            }
            if !RegKind::unify(&mut reg_kinds[b], ka) {
                return None;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
