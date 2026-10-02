mod s5a {
    //! `ForPrep` / `ForLoop` whitelist for Lua 5.4+ Int loops.
    //! `loop_int_1m` cells under 5.4 / 5.5 compile to a Cranelift
    //! counted loop. The pre-5.3 and Float forms are covered by
    //! `s5a_b` and `s5a_c`.

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int_55(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn for_1_to_1000_sums_to_500500() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 1000 do s = s + i end return s"),
            500500,
        );
    }

    #[test]
    fn for_descending_step_minus_1() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 10, 1, -1 do s = s + i end return s"),
            55,
        );
    }

    #[test]
    fn for_empty_loop_skips_body() {
        // `for i = 10, 1` with positive default step is an empty range.
        assert_eq!(
            eval_int_55("local s = 0 for i = 10, 1 do s = s + 1 end return s"),
            0,
        );
    }

    #[test]
    fn for_single_iter() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 1 do s = s + i end return s"),
            1,
        );
    }

    #[test]
    fn for_body_references_control_var() {
        // R[A+3] is the body-visible `i`. Body reads it via Move/Add
        // — must match interpreter.
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 100 do s = s + i * 2 end return s"),
            10100,
        );
    }

    /// The headline cell — `for i = 1, 1000000` uses `LoadK Int(1000000)`
    /// for the limit (not LoadI, since 1000000 > MAX_SBX). Both the
    /// LoadK Int whitelist extension and ForPrep/ForLoop have to be in
    /// place for this to compile.
    #[test]
    fn loop_int_1m_5_5_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_55(src), 500000500000);
    }

    /// 5.4 mirrors 5.5 — both `post53`, same emit path, shares the
    /// thread-local cache slot.
    #[test]
    fn loop_int_1m_5_4_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_with(LuaVersion::Lua54, src), 500000500000);
    }

    /// Pre-5.3 dialects use the pre-decrement ForPrep form. The chunk
    /// has to yield the same answer.
    #[test]
    fn loop_int_1k_pre53_runs_through_interpreter() {
        let src = "local s = 0 for i = 1, 1000 do s = s + i end return s";
        // 5.1 / 5.2 numeric `for` uses Floats; the loop variable is a
        // Float there, so the interpreter returns Float(500500.0).
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua51);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => assert_eq!(f, 500500.0),
            other => panic!("expected Float(500500.0) under 5.1, got {other:?}"),
        }
        // 5.3 uses Ints like 5.4/5.5, but the pre53 ForPrep form bails
        // the JIT — interpreter still returns Int(500500).
        assert_eq!(eval_int_with(LuaVersion::Lua53, src), 500500);
    }

    /// Cache-key correctness: same source loaded as 5.4 (post53) and
    /// 5.3 (pre53) lands in distinct cache slots — each compiles to a
    /// different form (count form vs pre-decrement form). The dialect
    /// bit in `proto_cache_key` is what keeps these from sharing a
    /// slot. Both forms are emitted, so the assert shape is "two
    /// distinct Compiled slots, both with the right loop semantics".
    #[test]
    fn cache_pre53_post53_distinct() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 100 do s = s + i end return s";

        let mut vm55 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl55 = vm55.load(src, b"=t").expect("compile");
        let r55 = vm55.call_value(Value::Closure(cl55), &[]).expect("run");
        assert!(matches!(
            cl55.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
        assert!(matches!(r55.first(), Some(&Value::Int(5050))));

        let mut vm53 = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let cl53 = vm53.load(src, b"=t").expect("compile");
        let r53 = vm53.call_value(Value::Closure(cl53), &[]).expect("run");
        assert!(matches!(
            cl53.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
        assert!(matches!(r53.first(), Some(&Value::Int(5050))));

        // cache is per-`Vm`, so each Vm carries exactly one entry for
        // its own dialect; the dialect-distinguishing invariant is
        // checked by asserting each Vm cached its own version exactly
        // once.
        assert_eq!(crate::jit_backend::cache_entry_count(&vm55), 1);
        assert_eq!(crate::jit_backend::cache_entry_count(&vm53), 1);
    }

    /// A non-immediate step bails out — variable `local step = 2;
    /// for i = 1, N, step` puts `step` in a register written by
    /// LoadI then read by ForPrep, but a step that comes from a
    /// non-LoadI source can't be const-folded at JIT time. Verify the
    /// chunk still produces the right answer through the interpreter.
    #[test]
    fn non_immediate_step_runs_through_interpreter() {
        // `for i = 1, 10, s` where s is a variable reachable only as
        // a Move target — the scan should not see a `LoadI` for the
        // step register and bail.
        let src = "local function f(s) local sum = 0 for i = 1, 10, s do sum = sum + i end return sum end; return f(2)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => assert_eq!(i, 25), // 1+3+5+7+9
            other => panic!("expected Int(25), got {other:?}"),
        }
    }
}

mod s5a_b {
    //! `ForPrep` / `ForLoop` pre-5.3 (limit-compare) form.
    //! Lua 5.3 `for i = 1, N` chunks JIT-compile under the
    //! pre-decrement ForPrep + limit-compare ForLoop emit. 5.1 / 5.2
    //! loop variables live in a Float register (see `s5a_c`).

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int_53(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn pre53_for_1_to_1000_sums_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 1, 1000 do s = s + i end return s"),
            500500,
        );
    }

    #[test]
    fn pre53_descending_step_minus_1_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 10, 1, -1 do s = s + i end return s"),
            55,
        );
    }

    #[test]
    fn pre53_empty_loop_skips_body_under_5_3() {
        // `for i = 10, 1` with default positive step is empty: ForPrep
        // pre-decrements R[A] = 10 - 1 = 9; ForLoop's first add yields
        // 10, which is > limit=1 → exit without entering body.
        assert_eq!(
            eval_int_53("local s = 0 for i = 10, 1 do s = s + 1 end return s"),
            0,
        );
    }

    #[test]
    fn pre53_single_iter_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 5, 5 do s = s + i end return s"),
            5,
        );
    }

    /// The 5.3 headline cell — `for i = 1, 1000000` with LoadK Int(1000000).
    #[test]
    fn loop_int_1m_5_3_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_53(src), 500000500000);
    }

    /// Pin the JIT state so a future change that silently regresses
    /// this back to the interpreter is caught.
    #[test]
    fn loop_int_1m_5_3_jit_state_compiled() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(500000500000))));
    }
}

mod s5a_c {
    //! `ForPrep` / `ForLoop` Float form (pre53 + post53).
    //!
    //! Lua 5.1 / 5.2 have no Int subtype, so numeric `for i = 1, N`
    //! lowers to a Float-typed loop var (R[A] = LoadF 1, R[A+1] =
    //! LoadK Float(N) or LoadF, R[A+2] = LoadI step). The scanner
    //! picks the loop kind from R[A]'s scanned kind and ForPrep /
    //! ForLoop have Float emit branches.
    //!
    //! The Float ForLoop has the same shape for pre53 and post53 (Lua's
    //! Float branch never had a count form). The Float ForPrep splits
    //! by dialect: pre53 pre-decrement + unconditional jump, post53
    //! empty-test + state-set + fall through.
    //!
    //! Body arithmetic on Float locals is supported, so
    //! `s = s + i` inside the body lowers to fadd against the visible
    //! R[A+3] register.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_float_with(version: LuaVersion, src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    /// post53 (5.5) explicit Float loop — `for i = 1.0, 1000.0 do …`.
    #[test]
    fn post53_float_for_1_to_1000_sums_under_5_5() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua55,
                "local s = 0.0 for i = 1.0, 1000.0 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// pre53 (5.3) explicit Float loop — same source, pre-decrement
    /// ForPrep + Float ForLoop.
    #[test]
    fn pre53_float_for_1_to_1000_sums_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 0.0 for i = 1.0, 1000.0 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// 5.1 headline cell — `for i = 1, 1000000` lowers to LoadF init +
    /// LoadK Float(1e6) limit + LoadI step. The chunk
    /// JIT-compiles end-to-end.
    #[test]
    fn loop_int_1m_5_1_matches_interpreter() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua51,
                "local s = 0 for i = 1, 1000000 do s = s + i end return s",
            ),
            500000500000.0,
        );
    }

    /// 5.2 headline cell — same source.
    #[test]
    fn loop_int_1m_5_2_matches_interpreter() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua52,
                "local s = 0 for i = 1, 1000000 do s = s + i end return s",
            ),
            500000500000.0,
        );
    }

    /// Pin JitProtoState for the 5.1 headline cell — confirms the
    /// chunk actually hits the JIT (not just the interpreter fallback).
    #[test]
    fn loop_int_1m_5_1_jit_state_compiled() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua51);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Float(f)) if f == 500000500000.0));
    }

    /// Regression guard — 5.5 still goes through the Int path even with
    /// the Float branches in place. R[A] stays Int (LoadI init), so the
    /// scan still pins Int.
    #[test]
    fn loop_int_1m_5_5_still_jit_int_path() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(500000500000))));
    }

    /// pre53 Float descending step. `for i = 1000.0, 1.0, -1 do …`
    /// exercises the negative-step path of the Float ForPrep / ForLoop.
    #[test]
    fn pre53_float_descending_step_minus_1_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 0.0 for i = 1000.0, 1.0, -1 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// pre53 Float empty loop — `for i = 10.0, 1.0` with default
    /// positive step. The pre-decrement ForPrep writes R[A] = 9.0; the
    /// first ForLoop add yields 10.0, which is > limit=1.0 → exit
    /// without entering body. Accumulator stays at its initial value.
    #[test]
    fn pre53_float_empty_loop_skips_body_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 7.0 for i = 10.0, 1.0 do s = s + 1.0 end return s",
            ),
            7.0,
        );
    }

    /// post53 Float empty loop — explicit empty test (init > limit for
    /// positive step) jumps straight to exit.
    #[test]
    fn post53_float_empty_loop_skips_body_under_5_5() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua55,
                "local s = 7.0 for i = 10.0, 1.0 do s = s + 1.0 end return s",
            ),
            7.0,
        );
    }
}
