use super::*;

/// What the whitelist scan learns about a chunk.
pub(super) struct ChunkScan {
    pub(super) allows_self_recursion: bool,
    pub(super) self_upval_idx: Option<u32>,
    pub(super) bb_starts: Vec<bool>,
    pub(super) sees_return1: bool,
    pub(super) self_upval: Vec<bool>,
    pub(super) is_upval_value_read: Vec<bool>,
    pub(super) self_call_pcs: Vec<bool>,
    pub(super) step_const: Vec<Option<i64>>,
    pub(super) for_loops: Vec<(usize, usize, i64)>,
    pub(super) defines_table: Vec<bool>,
    pub(super) folded_math: Vec<bool>,
    pub(super) math_folds: Vec<MathFold>,
}

pub(super) fn scan_chunk(c: ChunkIn<'_>) -> Option<ChunkScan> {
    let ChunkIn {
        proto,
        code,
        n,
        num_params,
        max_stack,
        float_only,
        ..
    } = c;
    // luna's `local function f(...) end` idiom binds upvalue 0
    // (Lua 5.5/5.4/5.3/5.2) or upvalue 1 (Lua 5.1 — slot 0 is the
    // `_ENV` placeholder) to the closure itself. Upvalue
    // tracking is general: the scanner watches GetUpval(b) and pins the
    // self-upval index from the first occurrence; subsequent
    // GetUpval(b') with b' != self-upval-idx bails. Upvals count is
    // bounded only to avoid pathological cases.
    let allows_self_recursion = !proto.upvals.is_empty() && proto.upvals.len() <= 4;
    let self_upval_idx: Option<u32> = None;
    let mut bb_starts = vec![false; n];
    bb_starts[0] = true;
    let sees_return1 = false;
    // Per-register "this slot last held a self-upval-loaded closure"
    // tag. Carried across Move; cleared by any other writer. Lookup
    // at Op::Call decides whether it's a self-recursive call we can
    // lower. Indexed by Lua register number.
    let self_upval: Vec<bool> = vec![false; max_stack];
    // per-PC role for `Op::GetUpval`. SelfMarker (true at
    // the bool position is misleading — see the enum-like split below)
    // is the call-target shortcut; ValueRead enables fetching the
    // upvalue value at runtime via `luna_jit_upval_get` so chunks like
    // `function () return k * k end` can JIT. `is_upval_value_read[pc]`
    // is true iff the role is ValueRead. Pre-pass below decides via
    // an 8-op lookahead from each `GetUpval`.
    let is_upval_value_read: Vec<bool> = determine_getupval_roles(code);
    // PC of every Op::Call that resolves to the self-recursion edge.
    // Emit-side consumes this to lower as a cranelift `call fn_id`.
    let self_call_pcs: Vec<bool> = vec![false; n];
    // track each register's last-written `LoadI` immediate (or
    // None when it was overwritten by anything else). `ForPrep` reads
    // `step_const[A+2]` to check that the step is a compile-time
    // constant ≠ 0 — non-immediate steps bail to the interpreter.
    let step_const: Vec<Option<i64>> = vec![None; max_stack];
    // every JIT'd ForPrep/ForLoop pair, in source order. Each
    // tuple is `(prep_pc, loop_pc, step_imm)`. Emit consumes this to
    // lay out the counted-loop blocks.
    let for_loops: Vec<(usize, usize, i64)> = Vec::new();

    // `defines_table[reg]` tracks whether a `NewTable` or
    // `Move` from a defined table reg has run by the current scan
    // position. Reset on any non-table-producing write to the
    // register. SetTable / GetI / Len require the operand to be
    // marked.
    //
    // Limitation: this is a single forward pass without BB-level
    // intersection at join points. To stay correct in the presence
    // of conditional branching, we bail any chunk that has both a
    // `NewTable` AND any conditional op (`Lt` / `Le` / `Eq`) — see
    // the `has_conditional` / `has_new_table` end check below.
    // Without that restriction a conditional NewTable would be
    // represented at the SetTable use site by a cranelift phi node
    // merging the table ptr with the entry-block iconst(0), and
    // the false-branch path would feed NULL into the Rust helper.
    let mut defines_table: Vec<bool> = vec![false; max_stack];
    // function params are guaranteed defined by the
    // caller. The dispatcher's `try_jit_call_op` only marshals
    // `Value::Table` into a Table-typed slot (via `arg_table_mask`),
    // so a Table-typed param truly holds a valid `Gc<Table>` ptr at
    // entry. Treat all params as table-defined upfront; the RegKind
    // sweep still rejects a non-Table param being used as a table
    // (the kind mismatch surfaces there as a unify failure).
    for i in 0..num_params {
        if let Some(slot) = defines_table.get_mut(i) {
            *slot = true;
        }
    }

    // pre-scan: detect `math.<fn>(arg)` 4-op folds. The
    // pattern is dialect-invariant — Lua 5.1 through 5.5 all emit
    // the same `GetTabUp / GetField / Move / Call` window for
    // `<env>.math.<fn>(<reg>)`. When a window matches, every
    // participating PC is marked `folded_math[pc] = true` so the
    // main whitelist loop below accepts them in-place; emit folds
    // them into a single cranelift libm call.
    //
    // Requires: `proto.upvals[0].name == "_ENV"`. luna's frontend
    // always parks the env upvalue at slot 0 (5.5/5.4/5.3/5.2: the
    // sole upvalue of any chunk; 5.1: an explicit `_ENV` placeholder
    // even though 5.1 source has no lexical `_ENV`). Any other shape
    // bails the fold for that PC.
    let mut folded_math: Vec<bool> = vec![false; n];
    let mut math_folds: Vec<MathFold> = Vec::new();
    let env_upval_present = proto
        .upvals
        .first()
        .map(|u| &*u.name == "_ENV")
        .unwrap_or(false);
    if env_upval_present {
        let mut try_pc = 0usize;
        while try_pc + 3 < n {
            if let Some(fold) = try_match_math_fold(&proto, code, try_pc, float_only) {
                folded_math[try_pc] = true;
                folded_math[try_pc + 1] = true;
                folded_math[try_pc + 2] = true;
                folded_math[try_pc + 3] = true;
                math_folds.push(fold);
                try_pc += 4;
            } else {
                try_pc += 1;
            }
        }
    }
    // The folds are checked once, at entry; a table store in the body
    // could reassign a math field after that.
    if !math_folds.is_empty()
        && code.iter().any(|i| {
            matches!(
                i.op(),
                Op::SetTable | Op::SetI | Op::SetField | Op::SetTabUp
            )
        })
    {
        return None;
    }

    let mut s = ChunkScan {
        allows_self_recursion,
        self_upval_idx,
        bb_starts,
        sees_return1,
        self_upval,
        is_upval_value_read,
        self_call_pcs,
        step_const,
        for_loops,
        defines_table,
        folded_math,
        math_folds,
    };
    let mut pc = 0;
    while pc < n {
        let ins = code[pc];
        match ins.op() {
            Op::LoadI
            | Op::LoadF
            | Op::LoadK
            | Op::LoadNil
            | Op::Move
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div => scan_data(&mut s, c, ins)?,
            Op::GetUpval | Op::Call | Op::GetTabUp | Op::GetField => {
                scan_ops::scan_calls(&mut s, c, pc, ins)?
            }
            Op::Return1
            | Op::Return0
            | Op::Jmp
            | Op::Lt
            | Op::Le
            | Op::Eq
            | Op::ForPrep
            | Op::ForLoop => pc = scan_ops::scan_control(&mut s, c, pc, ins)?,
            Op::NewTable | Op::SetTable | Op::SetList | Op::GetI | Op::GetTable | Op::Len => {
                scan_ops::scan_tables(&mut s, c, pc, ins)?
            }
            _ => return None,
        }
        pc += 1;
    }
    Some(s)
}

fn scan_data(s: &mut ChunkScan, c: ChunkIn<'_>, ins: Inst) -> Option<()> {
    let ChunkIn { proto, .. } = c;
    let ChunkScan {
        self_upval,
        step_const,
        defines_table,
        ..
    } = s;
    match ins.op() {
        Op::LoadI => {
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = Some(ins.sbx() as i64);
            }
        }
        Op::LoadF => {
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        Op::LoadK => {
            // Float constants pass. Int constants also
            // pass. Lua compilers reach for `LoadK Int(v)` when the
            // immediate doesn't fit in `LoadI`'s ±MAX_SBX range
            // (e.g. `for i = 1, 1000000` puts 1000000 in a
            // constant slot). String / Bool / Nil LoadK still bails.
            let bx = ins.bx() as usize;
            let k = proto.consts.get(bx).copied();
            if !matches!(k, Some(LuaValue::Float(_)) | Some(LuaValue::Int(_))) {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                // a `LoadK Int(v)` also pins the register
                // to a known compile-time constant. ForPrep can
                // use this register as its step source just like
                // a `LoadI`.
                *slot = match k {
                    Some(LuaValue::Int(v)) => Some(v),
                    _ => None,
                };
            }
        }
        Op::LoadNil => {
            // `R[A..=A+B] = nil`. The whitelist accepts
            // LoadNil for the cross_dialect `binary_trees` shape
            // (`{nil, nil}` leaf), where the freshly-NewTable'd
            // array slots are written nil by LoadNil and then
            // SetList-stored. Every Lua writer that LoadNil
            // overrides clears the per-reg trackers; downstream
            // SetList emit detects the Nil writer via the BB-local
            // `current_is_nil` shadow and tags `RAW_TAG_NIL` instead
            // of the default Int tag. Arith / cmp on a Nil-written
            // register would silently treat 0 as an Int value;
            // bail those readers below (in the kind sweep and the
            // arith linear-pass) instead of risking miscompile.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            for off in 0..=b {
                let r = a + off;
                if let Some(slot) = self_upval.get_mut(r) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(r) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(r) {
                    *slot = false;
                }
            }
        }
        Op::Move => {
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let tag = self_upval.get(b).copied().unwrap_or(false);
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = tag;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            // propagate table-defined-ness through
            // Move. Note this is a single-pass walk; the
            // fixed-point below catches cases where the Move
            // precedes the NewTable in source order (back-edge
            // through a loop).
            let src_def = defines_table.get(b).copied().unwrap_or(false);
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = src_def;
            }
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            // Reading a self-upval-tagged register in arith means the
            // GetUpval was a generic upvalue read (e.g., `n + 1` over
            // an outer-local upvalue), not the self-recursion shortcut.
            // Bail out — only the call-target case is handled.
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            if self_upval.get(b).copied().unwrap_or(false)
                || self_upval.get(c).copied().unwrap_or(false)
            {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
