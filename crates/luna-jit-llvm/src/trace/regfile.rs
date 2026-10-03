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
    /// Pointer to alloca slot `off`.
    fn reg_slot(&self, off: IntValue<'static>, name: &str) -> Option<PointerValue<'static>> {
        // SAFETY: `regs` is an alloca of `max_stack` slots, and every
        // `off` is below `max_stack`: the copy loops run `0..max_stack`,
        // and `try_compile_trace` rejects a record whose register
        // operands reach `max_stack` before anything is emitted
        unsafe {
            self.builder
                .build_in_bounds_gep(self.regs_ty, self.regs, &[self.zero, off], name)
                .ok()
        }
    }

    /// Pointer to slot `off` of the caller's `reg_state` buffer.
    fn state_slot(&self, off: IntValue<'static>, name: &str) -> Option<PointerValue<'static>> {
        // SAFETY: the dispatcher passes a `reg_state` buffer of the
        // trace's `window_size` (= `max_stack`) i64 slots, and the copy
        // loops, the only callers, run `off` over `0..max_stack`
        unsafe {
            self.builder
                .build_in_bounds_gep(self.i64_type, self.rs_ptr, &[off], name)
                .ok()
        }
    }

    /// Load from alloca slot `idx`.
    pub(super) fn load(&self, idx: u32, name: &str) -> Option<IntValue<'static>> {
        let off = self.i64_type.const_int(idx as u64, false);
        let slot = self.reg_slot(off, name)?;
        let v = self.builder.build_load(self.i64_type, slot, name).ok()?;
        Some(v.into_int_value())
    }

    /// Store into alloca slot `idx`.
    pub(super) fn store(&self, idx: u32, val: IntValue<'static>, name: &str) -> Option<()> {
        let off = self.i64_type.const_int(idx as u64, false);
        let slot = self.reg_slot(off, name)?;
        self.builder.build_store(slot, val).ok()?;
        Some(())
    }

    /// Emit `load reg_state[i] → regs[i]` for `i in 0..max_stack`.
    pub(super) fn load_from_state(&self) -> Option<()> {
        let (builder, i64_type) = (self.builder, self.i64_type);
        for i in 0..self.max_stack {
            let off = i64_type.const_int(i as u64, false);
            let src = self.state_slot(off, "rs_load_slot")?;
            let val = builder.build_load(i64_type, src, "rs_val").ok()?;
            let dst = self.reg_slot(off, "reg_slot")?;
            builder.build_store(dst, val).ok()?;
        }
        Some(())
    }

    /// Emit `store regs[i] → reg_state[i]` for `i in 0..max_stack` into the
    /// builder's current block. Shared by the clean-tail and every side-exit BB.
    pub(super) fn store_back(&self) -> Option<()> {
        let (builder, i64_type) = (self.builder, self.i64_type);
        for i in 0..self.max_stack {
            let off = i64_type.const_int(i as u64, false);
            let src = self.reg_slot(off, "sb_src")?;
            let val = builder.build_load(i64_type, src, "sb_val").ok()?;
            let dst = self.state_slot(off, "sb_dst")?;
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
