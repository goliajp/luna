//! The trace function's alloca register file and its copies to and from
//! the caller's `reg_state` buffer.

use inkwell::IntPredicate;
use inkwell::builder::Builder;
use inkwell::types::{ArrayType, IntType};
use inkwell::values::{IntValue, PointerValue};

pub(super) struct RegFile<'b> {
    pub(super) builder: &'b Builder<'static>,
    pub(super) i64_type: IntType<'static>,
    pub(super) regs_ty: ArrayType<'static>,
    pub(super) regs: PointerValue<'static>,
    pub(super) rs_ptr: PointerValue<'static>,
    pub(super) max_stack: usize,
    pub(super) zero: IntValue<'static>,
}

impl RegFile<'_> {
    /// Load from alloca slot `idx`.
    pub(super) fn load(&self, idx: u32, name: &str) -> Option<IntValue<'static>> {
        let off = self.i64_type.const_int(idx as u64, false);
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(self.regs_ty, self.regs, &[self.zero, off], name)
                .ok()?
        };
        let v = self.builder.build_load(self.i64_type, slot, name).ok()?;
        Some(v.into_int_value())
    }

    /// Store into alloca slot `idx`.
    pub(super) fn store(&self, idx: u32, val: IntValue<'static>, name: &str) -> Option<()> {
        let off = self.i64_type.const_int(idx as u64, false);
        let slot = unsafe {
            self.builder
                .build_in_bounds_gep(self.regs_ty, self.regs, &[self.zero, off], name)
                .ok()?
        };
        self.builder.build_store(slot, val).ok()?;
        Some(())
    }

    /// Emit `load reg_state[i] → regs[i]` for `i in 0..max_stack`.
    pub(super) fn load_from_state(&self) -> Option<()> {
        let builder = self.builder;
        let (i64_type, regs_ty, regs, rs_ptr, zero) = (
            self.i64_type,
            self.regs_ty,
            self.regs,
            self.rs_ptr,
            self.zero,
        );
        for i in 0..self.max_stack {
            let off = i64_type.const_int(i as u64, false);
            let src = unsafe {
                builder
                    .build_in_bounds_gep(i64_type, rs_ptr, &[off], "rs_load_slot")
                    .ok()?
            };
            let val = builder.build_load(i64_type, src, "rs_val").ok()?;
            let dst = unsafe {
                builder
                    .build_in_bounds_gep(regs_ty, regs, &[zero, off], "reg_slot")
                    .ok()?
            };
            builder.build_store(dst, val).ok()?;
        }
        Some(())
    }

    /// Emit `store regs[i] → reg_state[i]` for `i in 0..max_stack` into the
    /// builder's current block. Shared by the clean-tail and every side-exit BB.
    pub(super) fn store_back(&self) -> Option<()> {
        let builder = self.builder;
        let (i64_type, regs_ty, regs, rs_ptr, zero) = (
            self.i64_type,
            self.regs_ty,
            self.regs,
            self.rs_ptr,
            self.zero,
        );
        for i in 0..self.max_stack {
            let off = i64_type.const_int(i as u64, false);
            let src = unsafe {
                builder
                    .build_in_bounds_gep(regs_ty, regs, &[zero, off], "sb_src")
                    .ok()?
            };
            let val = builder.build_load(i64_type, src, "sb_val").ok()?;
            let dst = unsafe {
                builder
                    .build_in_bounds_gep(i64_type, rs_ptr, &[off], "sb_dst")
                    .ok()?
            };
            builder.build_store(dst, val).ok()?;
        }
        Some(())
    }

    /// Lua floor-mod: sign of result matches divisor.
    pub(super) fn floor_mod(
        &self,
        lhs: IntValue<'static>,
        rhs: IntValue<'static>,
    ) -> Option<IntValue<'static>> {
        let builder = self.builder;
        let raw = builder.build_int_signed_rem(lhs, rhs, "mod_srem").ok()?;
        let zero_v = self.i64_type.const_zero();
        let nonzero = builder
            .build_int_compare(IntPredicate::NE, raw, zero_v, "mod_nonzero")
            .ok()?;
        let xor = builder.build_xor(raw, rhs, "mod_xor").ok()?;
        let sign_differ = builder
            .build_int_compare(IntPredicate::SLT, xor, zero_v, "mod_signdif")
            .ok()?;
        let need_fix = builder.build_and(nonzero, sign_differ, "mod_fix").ok()?;
        let fixed = builder.build_int_add(raw, rhs, "mod_fixed").ok()?;
        let res = builder
            .build_select(need_fix, fixed, raw, "mod_res")
            .ok()?
            .into_int_value();
        Some(res)
    }
}
