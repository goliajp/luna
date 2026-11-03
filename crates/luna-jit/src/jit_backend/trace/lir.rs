//! The baseline tier: the trace lowering recorded as a compact instruction
//! list ([`Lir`]) and turned into machine code in one liveness pass, one
//! linear-scan allocation and one emission pass, without Cranelift.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};

mod alloc;
mod blocks;
mod build;
mod cg;
mod cg_ops;
mod clif;
mod code;
mod dump;
mod live;
#[cfg(feature = "llvm-jit")]
mod llvm;
mod masm;
mod pmove;
mod record;
mod spare;

#[cfg(all(target_arch = "aarch64", not(windows)))]
mod a64;
#[cfg(target_arch = "x86_64")]
mod x64;

pub(crate) use clif::define as define_clif;
pub(crate) use live::Start;
pub(crate) use code::{CodeArena, assemble};
#[cfg(feature = "llvm-jit")]
pub(crate) use llvm::compile as compile_llvm;

/// No value / no block.
pub(crate) const NONE: u32 = u32::MAX;

/// The type of a value. Narrow integers are held zero-extended in 64-bit
/// registers; `F64` lives in the floating-point registers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ty {
    I8,
    I16,
    I32,
    I64,
    F64,
}

impl Ty {
    pub(crate) fn of(t: cranelift_codegen::ir::Type) -> Ty {
        use cranelift_codegen::ir::types;
        match t {
            types::I8 => Ty::I8,
            types::I16 => Ty::I16,
            types::I32 => Ty::I32,
            types::I64 => Ty::I64,
            types::F64 => Ty::F64,
            other => panic!("trace lowering emitted an unsupported type {other}"),
        }
    }
    pub(crate) fn is_float(self) -> bool {
        self == Ty::F64
    }
    /// Width in bits of an integer type.
    pub(crate) fn bits(self) -> u32 {
        match self {
            Ty::I8 => 8,
            Ty::I16 => 16,
            Ty::I32 => 32,
            Ty::I64 | Ty::F64 => 64,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Sdiv,
    Umulhi,
    Smin,
    Smax,
    And,
    Or,
    Xor,
    Shl,
    Ushr,
    Sshr,
    Fadd,
    Fsub,
    Fmul,
    Fdiv,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UnOp {
    Ineg,
    Bnot,
    Fneg,
    Floor,
    Ceil,
    /// Zero-extension: values are already held zero-extended.
    Uextend,
    Ireduce,
    /// Bits moved between the integer and floating-point registers.
    Bitcast,
    FcvtFromSint,
    /// Truncation; the lowerer range-checks the operand first.
    FcvtToSint,
    FcvtToSintSat,
}

/// One recorded instruction. Operands are value numbers; `ty` is the
/// result type, or the operand type for comparisons and stores.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    Iconst(i64),
    /// The address of relocation `n` ([`Lir::relocs`]): never folded into
    /// another instruction, and emitted in a form another Vm's address can
    /// be written over.
    Reloc(u32),
    Fconst(u64),
    Bin(BinOp),
    /// `a op imm`; `c` is the constant's value when the lowerer passed
    /// one (the Cranelift replay reuses it), else `NONE`.
    BinImm(BinOp, i64),
    Un(UnOp),
    Icmp(IntCC),
    /// `a cc imm`; `c` as for `BinImm`.
    IcmpImm(IntCC, i64),
    Fcmp(FloatCC),
    /// `a ? b : c`
    Select,
    /// `*(a + off)`, zero-extended to `ty`'s register. `c` is `1` when the
    /// lowerer marked the access trusted (the replay keeps its flags).
    Load(i32),
    /// `*(u8 *)(a + off)`; `c` as for `Load`.
    Uload8(i32),
    /// `*(b + off) = a`; `c` as for `Load`.
    Store(i32),
    StackAddr(u32, i32),
    StackLoad(u32, i32),
    /// `slot[off] = a`
    StackStore(u32, i32),
    /// To block `a` with arguments `args`.
    Jump,
    /// On `a` to block `b` (arguments `args[..n_then]`), else to block `c`.
    Brif(u32),
    /// Function `a` (a declared function's index) with `args`.
    Call,
    /// The function at value `a`, of signature `b`, with `args`.
    CallIndirect,
    /// Returns value `a`.
    Return,
    /// `*(u32 *)cell += 1`, then to block `b` when it equals `at`, else
    /// to block `c`; `cell` is relocation `n`.
    TierCount {
        n: u32,
        at: u32,
    },
    /// The value of variable `a`.
    VarRead,
    /// Variable `a` takes value `b`.
    VarWrite,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Inst {
    pub(crate) op: Op,
    pub(crate) ty: Ty,
    /// The value defined, or `NONE`.
    pub(crate) dst: u32,
    pub(crate) a: u32,
    pub(crate) b: u32,
    pub(crate) c: u32,
    /// `args_at..args_at + n_args` in [`Lir::args`].
    pub(crate) args_at: u32,
    pub(crate) n_args: u32,
    /// The next instruction of the same block, or `NONE`.
    pub(crate) next: u32,
    /// `dst` as the lowerer sees it.
    pub(crate) res: cranelift_codegen::ir::Value,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockData {
    pub(crate) first: u32,
    pub(crate) last: u32,
    /// Instructions in the block.
    pub(crate) len: u32,
    /// `params_at..params_at + n_params` in [`Lir::bparams`].
    pub(crate) params_at: u32,
    pub(crate) n_params: u32,
    pub(crate) preds: u32,
    pub(crate) sealed: bool,
    pub(crate) entered: bool,
    /// The variables' values at the end of the only predecessor so far:
    /// `(offset, len)` in [`Lir::snaps`], offset `NONE` for none.
    pub(crate) inherit: (u32, u32),
}

/// Where the optimizing tier's loop starts: block `block`, which reads each
/// register `regs` names (register `i`'s variable) from `reg_state`; it goes
/// on to a loop head whose code took `inherit`, the value `(variable,
/// value)` each variable held on every back edge, which the loop then reads
/// in place of the variable.
#[derive(Clone, Debug)]
pub(crate) struct LoopEntry {
    pub(crate) block: u32,
    pub(crate) regs: Vec<u32>,
    pub(crate) inherit: Vec<(u32, u32)>,
}

/// A function a trace calls, or the signature of one it calls indirectly.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Callee {
    pub(crate) addr: usize,
    /// `params_at..params_at + n_params` in [`Lir::param_tys`].
    pub(crate) params_at: u32,
    pub(crate) n_params: u32,
    pub(crate) ret: Option<Ty>,
}

/// A trace function as the lowerer emitted it.
#[derive(Default)]
pub(crate) struct Lir {
    pub(crate) insts: Vec<Inst>,
    pub(crate) blocks: Vec<BlockData>,
    pub(crate) value_ty: Vec<Ty>,
    /// Per value: its value when it is an integer constant.
    pub(crate) konst: Vec<Option<i64>>,
    /// The Vm-specific addresses the code holds (see [`Op::Reloc`]).
    pub(crate) relocs: Vec<(super::RelocKind, i64)>,
    pub(crate) var_ty: Vec<Ty>,
    pub(crate) args: Vec<u32>,
    /// `(size, align_log2)` of each explicit stack slot.
    pub(crate) slots: Vec<(u32, u8)>,
    pub(crate) funcs: Vec<Callee>,
    pub(crate) sigs: Vec<Callee>,
    pub(crate) param_tys: Vec<Ty>,
    /// Block parameters (value numbers and as the lowerer sees them).
    pub(crate) bparams: Vec<u32>,
    pub(crate) bparam_vals: Vec<cranelift_codegen::ir::Value>,
    /// Snapshots of [`Lir::var_cur`] taken at branches.
    pub(crate) snaps: Vec<u32>,
    /// The function's parameter (`reg_state`).
    pub(crate) arg0: u32,
    pub(crate) cur: u32,
    pub(crate) var_cur: Vec<u32>,
    /// The values a block entered before being sealed inherits if it is
    /// sealed before anything is emitted into it.
    pub(crate) pending_inherit: Option<(u32, (u32, u32))>,
    /// Where the optimizing tier's loop starts (see `lower::handoff`).
    pub(crate) loop_entry: Option<LoopEntry>,
    /// `(block, snapshot)` of each block that took the values its
    /// predecessors agree on (see `Lir::edge`).
    pub(crate) inherited: Vec<(u32, (u32, u32))>,
    /// A primitive the backend does not implement was emitted.
    pub(crate) unsupported: Option<&'static str>,
    /// The helpers declared by the first trace this `Lir` recorded, and how
    /// many [`Lir::funcs`] / [`Lir::param_tys`] entries they take: they stay
    /// declared for the next trace.
    pub(in crate::jit_backend::trace) helpers: Option<(super::lower::Helpers, u32, u32)>,
}

impl Lir {
    pub(crate) fn block_params(&self, b: u32) -> &[u32] {
        let blk = &self.blocks[b as usize];
        &self.bparams[blk.params_at as usize..(blk.params_at + blk.n_params) as usize]
    }

    pub(crate) fn params(&self, c: &Callee) -> &[Ty] {
        &self.param_tys[c.params_at as usize..(c.params_at + c.n_params) as usize]
    }
}
