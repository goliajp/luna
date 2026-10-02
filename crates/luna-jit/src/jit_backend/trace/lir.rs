//! The baseline tier: the trace lowering recorded as a compact instruction
//! list ([`Lir`]) and turned into machine code in one liveness pass, one
//! linear-scan allocation and one emission pass, without Cranelift.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};

mod alloc;
mod build;
mod cg;
mod cg_ops;
mod code;
mod dump;
mod live;
mod pmove;

#[cfg(target_arch = "aarch64")]
mod a64;
#[cfg(target_arch = "x86_64")]
mod x64;

pub(crate) use code::{BaselineCode, assemble};

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
    Udiv,
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
    Fconst(u64),
    Bin(BinOp),
    BinImm(BinOp, i64),
    Un(UnOp),
    Icmp(IntCC),
    IcmpImm(IntCC, i64),
    Fcmp(FloatCC),
    /// `a ? b : c`
    Select,
    /// `*(a + off)`, zero-extended to `ty`'s register.
    Load(i32),
    /// `*(u8 *)(a + off)`
    Uload8(i32),
    /// `*(b + off) = a`
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
    pub(crate) params: Vec<u32>,
    pub(crate) param_vals: Vec<cranelift_codegen::ir::Value>,
    pub(crate) preds: u32,
    pub(crate) sealed: bool,
    pub(crate) entered: bool,
    /// The variables' values at the end of the only predecessor so far:
    /// `(offset, len)` in [`Lir::snaps`], offset `NONE` for none.
    pub(crate) inherit: (u32, u32),
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
    pub(crate) var_ty: Vec<Ty>,
    pub(crate) args: Vec<u32>,
    /// `(size, align_log2)` of each explicit stack slot.
    pub(crate) slots: Vec<(u32, u8)>,
    pub(crate) funcs: Vec<Callee>,
    pub(crate) sigs: Vec<Callee>,
    pub(crate) param_tys: Vec<Ty>,
    /// Snapshots of [`Lir::var_cur`] taken at branches.
    pub(crate) snaps: Vec<u32>,
    /// The function's parameter (`reg_state`).
    pub(crate) arg0: u32,
    pub(crate) cur: u32,
    pub(crate) var_cur: Vec<u32>,
    /// The values a block entered before being sealed inherits if it is
    /// sealed before anything is emitted into it.
    pub(crate) pending_inherit: Option<(u32, (u32, u32))>,
    /// A primitive the backend does not implement was emitted.
    pub(crate) unsupported: Option<&'static str>,
}

impl Lir {
    pub(crate) fn params(&self, c: &Callee) -> &[Ty] {
        &self.param_tys[c.params_at as usize..(c.params_at + c.n_params) as usize]
    }
}

thread_local! {
    static SPARE: std::cell::RefCell<Option<Lir>> = const { std::cell::RefCell::new(None) };
}

impl Lir {
    /// A cleared `Lir`, reusing the buffers of the last one given back on
    /// this thread.
    pub(crate) fn take() -> Lir {
        match SPARE.with(|s| s.borrow_mut().take()) {
            Some(mut l) => {
                l.insts.clear();
                l.blocks.clear();
                l.value_ty.clear();
                l.var_ty.clear();
                l.args.clear();
                l.slots.clear();
                l.funcs.clear();
                l.sigs.clear();
                l.param_tys.clear();
                l.snaps.clear();
                l.var_cur.clear();
                l.arg0 = NONE;
                l.cur = NONE;
                l.unsupported = None;
                l.pending_inherit = None;
                l
            }
            None => Lir::new(),
        }
    }

    pub(crate) fn give(self) {
        SPARE.with(|s| *s.borrow_mut() = Some(self));
    }
}
