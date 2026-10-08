//! The opcodes.

/// Opcode kinds for the luna bytecode. Layout follows PUC `lopcodes.h`
/// (5.5.0); semantics may differ where noted in the dispatcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Op {
    /// `R[A] := R[B]` register move.
    Move,
    /// `R[A] := sBx` load immediate integer.
    LoadI,
    /// `R[A] := (lua_Number)sBx` load immediate float.
    LoadF,
    /// `R[A] := K[Bx]` load constant.
    LoadK,
    /// `R[A] := K[extra_arg]` load constant with extended index (next op
    /// must be `ExtraArg`).
    LoadKx,
    /// `R[A] := false`.
    LoadFalse,
    /// `R[A] := false; pc++` load false and skip next instruction.
    LFalseSkip,
    /// `R[A] := true`.
    LoadTrue,
    /// `R[A..A+B] := nil` clear a register range.
    LoadNil,
    /// `R[A] := Upvalues[B]`.
    GetUpval,
    /// `Upvalues[B] := R[A]`.
    SetUpval,
    /// `R[A] := Upvalues[B][K[C]:string]` global-style table read on an
    /// upvalue.
    GetTabUp,
    /// `R[A] := R[B][R[C]]`.
    GetTable,
    /// `R[A] := R[B][C:int]` integer-indexed read.
    GetI,
    /// `R[A] := R[B][K[C]:string]` field read with constant key.
    GetField,
    /// `Upvalues[A][K[B]:string] := R[C]/K[C]`. In every table write `k`
    /// set means the stored value is the constant `K[C]` (as in PUC 5.4).
    SetTabUp,
    /// `R[A][R[B]] := R[C]/K[C]`.
    SetTable,
    /// `R[A][B:int] := R[C]/K[C]` integer-indexed write.
    SetI,
    /// `R[A][K[B]:string] := R[C]/K[C]` field write.
    SetField,
    /// `R[A] := {}` allocate a new table; B/C carry size hints.
    NewTable,
    /// `R[A+1] := R[B]; R[A] := R[B][K[C]:string]` self-method prep for
    /// `obj:m(...)`.
    SelfOp,
    /// `R[A] := R[B] + R[C]/K[C]`. With `k` set it is a 5.4+ `x - 0`, which
    /// PUC compiles as `ADDI x 0`: numbers add (so `-0.0 - 0` is `0.0`),
    /// anything else is subtracted, `__sub` and string coercion included.
    Add,
    /// `R[A] := R[B] - R[C]/K[C]`.
    Sub,
    /// `R[A] := R[B] * R[C]/K[C]`.
    Mul,
    /// `R[A] := R[B] % R[C]/K[C]`.
    Mod,
    /// `R[A] := R[B] ^ R[C]/K[C]`.
    Pow,
    /// `R[A] := R[B] / R[C]/K[C]`.
    Div,
    /// `R[A] := R[B] // R[C]/K[C]`.
    IDiv,
    /// `R[A] := R[B] & R[C]/K[C]`.
    BAnd,
    /// `R[A] := R[B] | R[C]/K[C]`.
    BOr,
    /// `R[A] := R[B] ~ R[C]/K[C]`.
    BXor,
    /// `R[A] := R[B] << R[C]/K[C]`.
    Shl,
    /// `R[A] := R[B] >> R[C]/K[C]`.
    Shr,
    /// `R[A] := -R[B]` arithmetic negation.
    Unm,
    /// `R[A] := ~R[B]` bitwise NOT.
    BNot,
    /// `R[A] := not R[B]`.
    Not,
    /// `R[A] := #R[B]` length operator.
    Len,
    /// `R[A] := R[A] .. ... .. R[A+B-1]` string concatenation chain.
    Concat,
    /// Close upvalues in scope `A` (closes pending `<close>` and upvalues).
    Close,
    /// Mark to-be-closed slot `A` (5.4).
    Tbc,
    /// `pc += sJ` unconditional jump.
    Jmp,
    /// Equality comparison with optional skip.
    Eq,
    /// Less-than comparison with optional skip.
    Lt,
    /// Less-or-equal comparison with optional skip.
    Le,
    /// Equality against a constant.
    EqK,
    /// `if (not R[A]) == k then pc++`.
    Test,
    /// `if (not R[B]) == k then pc++ else R[A] := R[B]`.
    TestSet,
    /// `R[A], ..., R[A+C-2] := R[A](R[A+1], ..., R[A+B-1])`.
    Call,
    /// Tail call (same register/return contract as `Call`).
    TailCall,
    /// `return R[A], ..., R[A+B-2]`.
    Return,
    /// `return` with no values.
    Return0,
    /// `return R[A]` single-value return.
    Return1,
    /// Numeric-for iteration step.
    ForLoop,
    /// Numeric-for prepare (validates types, normalizes step).
    ForPrep,
    /// Generic-for prepare.
    TForPrep,
    /// Generic-for call: invoke iterator once.
    TForCall,
    /// Generic-for loop tail (branch back if iterator returned non-nil).
    TForLoop,
    /// Bulk-store a sequence into a table (table constructor).
    SetList,
    /// `R[A] := closure(KPROTO[Bx])`.
    Closure,
    /// `R[A], R[A+1], ..., R[A+C-2] := vararg`.
    Vararg,
    /// 5.5: materialize the vararg table into `R[A]` (named vararg that is
    /// written / escapes / is `_ENV`). Builds it from the stack varargs.
    GetVarg,
    /// 5.5: `R[A] := vararg[R[C]]` — index the *virtual* named vararg without
    /// allocating a table. Integer key in `[1,n]` → that vararg, key `"n"` →
    /// the count, else nil (PUC OP_GETVARG on an unmaterialized vararg).
    VargIdx,
    /// 5.5: error if `R[A]` is not nil — a defining `global` write whose target
    /// already exists. Bx is the name constant index + 1 (0 ⇒ unknown name).
    ErrNNil,
    /// Extended-immediate payload for the preceding instruction (see
    /// `LoadKx`).
    ExtraArg,
    // The constant- and immediate-operand forms (PUC 5.4 `ADDI`, `ADDK`…,
    // `EQI`…). They come after `ExtraArg` so that the opcodes above keep
    // their numbers. An arithmetic one falls back to the metamethod of its
    // own operator, on the operands in source order: with `k` set the
    // constant was the left operand.
    /// `R[A] := R[B] + sC`.
    AddI,
    /// `R[A] := R[B] - sC`.
    SubI,
    /// `R[A] := R[B] + K[C]:number`.
    AddK,
    /// `R[A] := R[B] - K[C]:number`.
    SubK,
    /// `R[A] := R[B] * K[C]:number`.
    MulK,
    /// `R[A] := R[B] % K[C]:number`.
    ModK,
    /// `R[A] := R[B] ^ K[C]:number`.
    PowK,
    /// `R[A] := R[B] / K[C]:number`.
    DivK,
    /// `R[A] := R[B] // K[C]:number`.
    IDivK,
    /// `R[A] := R[B] & K[C]:integer`.
    BAndK,
    /// `R[A] := R[B] | K[C]:integer`.
    BOrK,
    /// `R[A] := R[B] ~ K[C]:integer`.
    BXorK,
    /// `R[A] := R[B] >> sC`.
    ShrI,
    /// `R[A] := R[B] << sC` (PUC's `SHLI` is `sC << R[B]`; luna has no
    /// `MMBINI` to say which shift the source wrote).
    ShlI,
    /// `if ((R[A] == sB) ~= k) then pc++`; with `C` set the immediate is the
    /// float `sB`. Raw: no metamethod.
    EqI,
    /// `if ((R[A] < sB) ~= k) then pc++`; `C` as for `EqI`.
    LtI,
    /// `if ((R[A] <= sB) ~= k) then pc++`; `C` as for `EqI`.
    LeI,
    /// `if ((R[A] > sB) ~= k) then pc++`; `C` as for `EqI`.
    GtI,
    /// `if ((R[A] >= sB) ~= k) then pc++`; `C` as for `EqI`.
    GeI,
    /// `R[A] := R[B][K[C]]` for a constant key of any type: before 5.4 a
    /// `GETTABLE` key is an `RK` operand, so a number key takes no register.
    GetTableK,
    /// `R[A][K[B]] := R[C]/K[C]` for a constant key of any type (before
    /// 5.4 a `SETTABLE` key is an `RK` operand).
    SetTableK,
    /// `R[A] := Up[B][K[C]]` with `k` set, else `R[A] := Up[B][R[C]]`: a
    /// 5.2 / 5.3 `GETTABUP` whose key is not a string constant.
    GetTabUpR,
    /// `Up[A][R[B]] := R[C]/K[C]`: a 5.2 / 5.3 `SETTABUP` with its key in a
    /// register.
    SetTabUpR,
    /// `Up[A][K[B]] := R[C]/K[C]`: a 5.2 / 5.3 `SETTABUP` whose key is a
    /// constant of any type.
    SetTabUpK,
}
