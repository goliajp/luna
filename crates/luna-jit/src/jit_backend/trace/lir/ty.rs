//! The types of the values a trace's instructions hold.

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
