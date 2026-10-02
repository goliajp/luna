mod s2c_b {
    //! interpreter `Op::Call` fast path. When the target
    //! closure's Proto is cached as `Compiled { num_args > 0 }`
    //! AND every arg slot is `Value::Int`, `begin_call` skips the
    //! interpreter frame setup and runs the cached native fn
    //! in-place.

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn calls_jit_inner_one_arg() {
        assert_eq!(
            eval_int("local function add1(n) return n + 1 end; return add1(41)"),
            42
        );
    }

    #[test]
    fn calls_jit_inner_two_args() {
        assert_eq!(
            eval_int("local function f(a, b) return a * b + 1 end; return f(5, 7)"),
            36
        );
    }

    #[test]
    fn calls_jit_inner_with_branch() {
        let src = "local function clip(n) if n < 0 then return 0 end; return n end; \
             return clip(-3) + clip(7)";
        assert_eq!(eval_int(src), 7);
    }

    #[test]
    fn multiple_calls_share_cache() {
        // The Proto is compiled once; the second call hits the cached
        // entry without recompiling.
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm
            .eval(
                "local function add1(n) return n + 1 end; \
                 return add1(10) + add1(20) + add1(30)",
            )
            .unwrap();
        // 11 + 21 + 31 = 63
        assert!(matches!(v.first(), Some(Value::Int(63))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "Proto compiled exactly once"
        );
    }

    #[test]
    fn jit_failed_state_falls_through() {
        // String body — lowerer bails; interpreter runs the chunk.
        assert!(
            crate::jit_backend::test_vm_new(LuaVersion::Lua55)
                .eval("local function f(n) return tostring(n) end; return #f(42)")
                .unwrap()
                .first()
                .map(|v| matches!(v, Value::Int(_)))
                .unwrap_or(false),
        );
    }
}

mod s2c_c {
    //! self-recursion through `Op::GetUpval(0)` + `Op::Call`.
    //! fib is the canonical shape; the lowerer recognises the paired
    //! ops and emits a direct cranelift `call` to the current fn,
    //! sidestepping any actual upvalue load.

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn fib_recursive_small() {
        let fib = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib";
        assert_eq!(
            eval_int(
                &format!("{fib} return fib(0)")
                    .replace("return fib return fib(0)", "; return fib(0)")
            ),
            0,
        );
    }

    #[test]
    fn fib_10_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(10)";
        assert_eq!(eval_int(src), 55);
    }

    #[test]
    fn fib_15_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(15)";
        assert_eq!(eval_int(src), 610);
    }

    #[test]
    fn fib_28_matches_interpreter() {
        // The classic baseline workload — must match the interpreter
        // value exactly. If this is wrong, every BASELINE cell movement
        // is meaningless.
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        assert_eq!(eval_int(src), 317811);
    }

    #[test]
    fn recursive_one_compile_per_proto() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm
            .eval(
                "local function fib(n) \
                   if n < 2 then return n end \
                   return fib(n - 1) + fib(n - 2) \
                 end; return fib(10)",
            )
            .unwrap();
        assert!(matches!(v.first(), Some(Value::Int(55))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "fib's Proto compiled exactly once"
        );
    }
}

mod s2c_c_perf_check {
    //! Sanity check that fib_28's Proto actually flips to Compiled.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    #[test]
    fn fib28_bench_source_flips_proto_to_compiled() {
        let src = "local function f(n) \
                     if n < 2 then return n end \
                     return f(n - 1) + f(n - 2) \
                   end \
                   return f(28)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).unwrap();
        assert!(matches!(v.first(), Some(Value::Int(317811))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "fib's Proto should compile exactly once",
        );
    }
}

mod s3 {
    //! Float fast path. Per-register type inference + Float
    //! arith / cmp lowerings + bitcast bookends at the i64 ABI
    //! boundary. fib_28 5.1/5.2 (Float-typed n) JIT-compiles
    //! end-to-end, matching the 5.3/5.4/5.5 path.

    use crate::jit_backend::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_float_55(src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    fn eval_with(version: LuaVersion, src: &str) -> Value {
        let mut vm = crate::jit_backend::test_vm_new(version);
        vm.eval(src)
            .expect("eval")
            .into_iter()
            .next()
            .expect("one value")
    }

    /// `return 1.5` const-folds to LoadK(Float(1.5)) — exercises the
    /// LoadK Float whitelist path.
    #[test]
    fn float_const_return() {
        assert_eq!(eval_float_55("return 1.5"), 1.5);
    }

    /// `return 1.5 + 2.5` const-folds again, but
    /// `local a=1.5; local b=2.5; return a+b` keeps two runtime LoadKs
    /// plus an Add — exercises the Float-arith path.
    #[test]
    fn float_runtime_arith() {
        assert_eq!(
            eval_float_55("local a = 1.5; local b = 2.5; return a + b"),
            4.0,
        );
        assert_eq!(
            eval_float_55("local a = 1.5; local b = 2.5; return a * b"),
            3.75,
        );
    }

    /// `local x=2.5; if x < 3.0 then return 1.0 else return 0.0 end`
    /// — exercises Float cmp (fcmp) + brif branching with Float regs.
    #[test]
    fn float_branch() {
        assert_eq!(
            eval_float_55("local x = 2.5; if x < 3.0 then return 1.0 else return 0.0 end"),
            1.0,
        );
        assert_eq!(
            eval_float_55("local x = 4.5; if x < 3.0 then return 1.0 else return 0.0 end"),
            0.0,
        );
    }

    /// Mixed Int + Float in one register sequence — the sweep should
    /// reject (`unify(Int, Float) → false`) and the chunk runs through
    /// the interpreter. `1 + 0.5` is const-folded by the parser so use
    /// runtime locals.
    #[test]
    fn mixed_int_float_bails_cleanly() {
        // Direct call to try_compile — should return None.
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        // R[0] = LoadI 1; R[1] = LoadK 0.5; Add R[2] = R[0]+R[1]
        let cl = vm
            .load(b"local a = 1; local b = 0.5; return a + b", b"=t")
            .expect("compile");
        // The scan unifies R[0]=Int with R[1]=Float in Add → bail.
        // (Or pins R[2]=Float and back-propagates — the unify rules
        // require both operands to agree, so this returns None.)
        assert!(try_compile_int_chunk(cl.proto, false, false).is_none());
        // And the interpreter still produces the correct result.
        let v = vm
            .eval("local a = 1; local b = 0.5; return a + b")
            .expect("eval");
        assert!(matches!(v.first(), Some(&Value::Float(1.5))));
    }

    /// fib_28 under Lua 5.2 — the inner closure's `n` is Float
    /// (5.2 has no integer subtype), and the JIT must take it via
    /// Float arg + Float ret.
    #[test]
    fn fib28_5_2_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua52, src) {
            Value::Float(f) => assert_eq!(f, 317811.0),
            other => panic!("expected Float(317811.0), got {other:?}"),
        }
    }

    /// fib_28 under Lua 5.1 — extra wrinkle: the inner closure has
    /// 2 upvals (`_ENV` placeholder at slot 0, `fib` self at slot 1).
    /// The scanner's `self_upval_idx` tracker should pin slot 1 from
    /// the first GetUpval(b=1) and lower the recursion correctly.
    #[test]
    fn fib28_5_1_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua51, src) {
            Value::Float(f) => assert_eq!(f, 317811.0),
            other => panic!("expected Float(317811.0), got {other:?}"),
        }
    }

    /// Sanity guard: 5.5 fib still goes through the Int path. The
    /// JIT cache slot ends up with `arg_float_mask: 0,
    /// ret_is_float: false` and the result is `Value::Int(317811)`.
    #[test]
    fn fib28_5_5_still_int() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua55, src) {
            Value::Int(i) => assert_eq!(i, 317811),
            other => panic!("expected Int(317811), got {other:?}"),
        }
    }

    /// Cache-key correctness regression: two protos with identical
    /// bytecode shape (`LoadK k0 + Return1`) but different `consts[0]`
    /// must not share a slot. Without `proto.consts` in the hash, the
    /// second proto would inherit the first's compiled constant.
    #[test]
    fn cache_key_includes_consts() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v1 = vm.eval("return 1.5").unwrap();
        assert!(matches!(v1.first(), Some(&Value::Float(f)) if f == 1.5));
        let v2 = vm.eval("return 2.5").unwrap();
        assert!(matches!(v2.first(), Some(&Value::Float(f)) if f == 2.5));
        // Two distinct compiled protos, two cache entries.
        assert_eq!(crate::jit_backend::cache_entry_count(&vm), 2);
    }

    /// 5.4 Division `a / b` always yields a Float in PUC semantics.
    /// `local a = 3.0; local b = 2.0; return a / b` should compile
    /// and produce 1.5.
    #[test]
    fn float_div() {
        assert_eq!(
            eval_float_55("local a = 3.0; local b = 2.0; return a / b"),
            1.5,
        );
    }
}
