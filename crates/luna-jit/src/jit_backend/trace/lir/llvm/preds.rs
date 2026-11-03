//! Cranelift's comparison codes as LLVM's predicates.

use super::*;
use inkwell::{FloatPredicate, IntPredicate};

pub(super) fn int_pred(cc: IntCC) -> IntPredicate {
    match cc {
        IntCC::Equal => IntPredicate::EQ,
        IntCC::NotEqual => IntPredicate::NE,
        IntCC::SignedLessThan => IntPredicate::SLT,
        IntCC::SignedGreaterThanOrEqual => IntPredicate::SGE,
        IntCC::SignedGreaterThan => IntPredicate::SGT,
        IntCC::SignedLessThanOrEqual => IntPredicate::SLE,
        IntCC::UnsignedLessThan => IntPredicate::ULT,
        IntCC::UnsignedGreaterThanOrEqual => IntPredicate::UGE,
        IntCC::UnsignedGreaterThan => IntPredicate::UGT,
        IntCC::UnsignedLessThanOrEqual => IntPredicate::ULE,
    }
}

pub(super) fn float_pred(cc: FloatCC) -> FloatPredicate {
    match cc {
        FloatCC::Ordered => FloatPredicate::ORD,
        FloatCC::Unordered => FloatPredicate::UNO,
        FloatCC::Equal => FloatPredicate::OEQ,
        FloatCC::NotEqual => FloatPredicate::UNE,
        FloatCC::OrderedNotEqual => FloatPredicate::ONE,
        FloatCC::UnorderedOrEqual => FloatPredicate::UEQ,
        FloatCC::LessThan => FloatPredicate::OLT,
        FloatCC::LessThanOrEqual => FloatPredicate::OLE,
        FloatCC::GreaterThan => FloatPredicate::OGT,
        FloatCC::GreaterThanOrEqual => FloatPredicate::OGE,
        FloatCC::UnorderedOrLessThan => FloatPredicate::ULT,
        FloatCC::UnorderedOrLessThanOrEqual => FloatPredicate::ULE,
        FloatCC::UnorderedOrGreaterThan => FloatPredicate::UGT,
        FloatCC::UnorderedOrGreaterThanOrEqual => FloatPredicate::UGE,
    }
}
