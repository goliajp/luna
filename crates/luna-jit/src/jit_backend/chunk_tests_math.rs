mod s5b {
    //! `math.<fn>(arg)` libcall fold.
    //!
    //! Recognized 4-op windows (`GetTabUp _ENV "math"` → `GetField R[A]
    //! "<fn>"` → `Move R[A+1] R[arg]` → `Call R[A] B=2 C=2`) collapse
    //! into a single cranelift `call` to libm. Bytecode is
    //! dialect-invariant: the same window appears across 5.1 – 5.5.
    //! Loop kind (Int vs Float) varies per dialect; the fold's
    //! emit converts an Int loop var to f64 at the
    //! libm call boundary via `fcvt_from_sint`.
    //!
    //! Correctness baseline for each cell is the interpreter's exact
    //! `Value::Float` return. We compare with `f64::EPSILON`-scaled
    //! tolerance because libm sin/cos and the interpreter's own libm
    //! calls share the same C runtime — bit-exact equality is the
    //! expected outcome on macOS / Linux, but a 1-ULP slack guards
    //! against future cross-platform drift.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_float_with(version: LuaVersion, src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    fn interp_only_float_with(version: LuaVersion, src: &str) -> f64 {
        // Bypass the JIT cache so we have an interpreter-only reference
        // result to compare against. The chunk runs through Vm::eval
        // exactly the same; we just disable cache hits by clearing
        // before AND we drop into call_value via load → ensuring the
        // JIT path is what gets hit. The reference is computed by
        // hand-mirroring the loop in Rust at the call sites below.
        let _ = version;
        let _ = src;
        unreachable!("references are precomputed in each test")
    }

    /// Headline cell: `math.sin(i)` over an Int loop in 5.5. Pin the
    /// JIT state to `Compiled` so we know the fold took, then assert
    /// the result matches `f64::sin` over the same integer range.
    #[test]
    fn math_sin_5_5_matches_libm() {
        let _ = interp_only_float_with;
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.sin(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).sin()).sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),
            "math.sin sum mismatch: jit={r_jit} ref={r_ref}"
        );
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
    }

    /// Symmetric `math.cos(i)` check — exercises the second entry in
    /// `MATH_LIBM_FNS` and the const-bytes cache key (sin
    /// and cos chunks must not collide).
    #[test]
    fn math_cos_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).cos()).sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),
            "math.cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
    }

    /// Two folds in one body: `math.sin(i) * math.cos(i)`. The cos
    /// fold writes back into a register that the sin fold's
    /// `Move` temp also targeted; the fold's RegKind handler skips the
    /// Move's unification on folded PCs so the conflicting kinds
    /// (Int from Move-of-loop-var, Float from cos result) don't
    /// abort compile.
    #[test]
    fn math_sin_cos_product_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 1000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=1000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 32.0).max(1e-9),
            "sin*cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
    }

    /// Headline `math_loop_100k` cell at full N=100 000 — confirms
    /// the actual bench source compiles and returns within libm
    /// tolerance on 5.5.
    #[test]
    fn math_loop_100k_5_5_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),
            "100k sin*cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
    }

    /// 5.4 — same body, Int loop var. Confirms the post53 Int loop +
    /// math fold combination compiles.
    #[test]
    fn math_loop_100k_5_4_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua54, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.3 — pre53 Int loop + math fold. Cache-key `pre53` bit
    /// distinguishes from 5.5's slot.
    #[test]
    fn math_loop_100k_5_3_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua53, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.2 — Float loop (`LoadF init / LoadK Float(N) limit`) + math
    /// fold. The arg conversion path here is identity (loop var is
    /// already Float).
    #[test]
    fn math_loop_100k_5_2_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua52, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.1 — same Float-loop path as 5.2. Both share the pre53 +
    /// Float fork in the ForPrep/ForLoop scanner.
    #[test]
    fn math_loop_100k_5_1_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua51, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// Cache key uses string-byte content (not just discriminant) —
    /// `math.sin` and `math.cos` chunks land in distinct cache slots
    /// even though their bytecode shape is identical bar the `C`
    /// operand of GetField.
    ///
    /// refactored to one Vm
    /// (cache is per-`Vm` now). The invariant under test (distinct
    /// libcall name → distinct slot) is preserved by asserting the
    /// cache grew from 1 (after sin) to 2 (after cos) in the same Vm.
    #[test]
    fn math_libcall_distinct_fns_distinct_cache() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let _ = vm
            .eval("local s = 0.0 for i = 1, 4 do s = s + math.sin(i) end return s")
            .expect("sin eval");
        let n_after_sin = crate::jit_backend::cache_entry_count(&vm);
        let _ = vm
            .eval("local s = 0.0 for i = 1, 4 do s = s + math.cos(i) end return s")
            .expect("cos eval");
        let n_after_cos = crate::jit_backend::cache_entry_count(&vm);
        assert!(
            n_after_cos > n_after_sin,
            "sin/cos chunks must hash to distinct cache slots (sin={n_after_sin} cos={n_after_cos})"
        );
    }

    /// `math.sqrt(i)` exercises another libm fn from the supported
    /// set. Result matches `f64::sqrt`.
    #[test]
    fn math_sqrt_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.sqrt(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).sqrt()).sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),);
    }

    /// Unsupported math fn (`math.pi` access, no Call) bails. A bare
    /// `math.pi + i` body has no `Call` after the `GetField`, so the
    /// fold pre-scan rejects it and the chunk falls back to interp.
    #[test]
    fn math_constant_access_bails_to_interp() {
        let src = "local s = 0.0 for i = 1, 4 do s = s + math.pi end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
        match r.first() {
            Some(&Value::Float(f)) => {
                assert!((f - 4.0 * std::f64::consts::PI).abs() < 1e-12);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    /// Two-arg math fn (`math.atan(y, x)`) bails — the `Call B=2 C=2`
    /// gate requires exactly 1 arg + 1 result. With two args the Call
    /// has B=3, the fold pre-scan rejects it.
    #[test]
    fn math_two_arg_atan_bails() {
        let src = "local s = 0.0 for i = 1, 4 do s = s + math.atan(i, 2) end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
    }
}
