mod smoke {
    use cranelift::prelude::*;
    use cranelift_codegen::ir::UserFuncName;
    use cranelift_frontend::FunctionBuilderContext;
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{Linkage, Module};

    /// Smoke test: hand-build recursive
    /// `fib(n: i64) -> i64` directly in cranelift IR, mmap-execute,
    /// and assert fib(28) == 317811.
    #[test]
    fn cranelift_jit_fib28_returns_317811() {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        let isa = cranelift_native::builder()
            .expect("host isa builder")
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let mut module = JITModule::new(builder);

        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let fib_id = module
            .declare_function("fib", Linkage::Local, &sig)
            .expect("declare fib");

        let mut ctx = module.make_context();
        ctx.func.signature = sig.clone();
        ctx.func.name = UserFuncName::user(0, fib_id.as_u32());

        let mut fbc = FunctionBuilderContext::new();
        let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

        let entry = bcx.create_block();
        let then_blk = bcx.create_block();
        let else_blk = bcx.create_block();
        bcx.append_block_params_for_function_params(entry);
        let n = bcx.block_params(entry)[0];
        bcx.switch_to_block(entry);
        bcx.seal_block(entry);

        let two = bcx.ins().iconst(types::I64, 2);
        let cmp = bcx.ins().icmp(IntCC::SignedLessThan, n, two);
        bcx.ins().brif(cmp, then_blk, &[], else_blk, &[]);

        bcx.switch_to_block(then_blk);
        bcx.seal_block(then_blk);
        bcx.ins().return_(&[n]);

        bcx.switch_to_block(else_blk);
        bcx.seal_block(else_blk);
        let one = bcx.ins().iconst(types::I64, 1);
        let n_minus_1 = bcx.ins().isub(n, one);
        let n_minus_2 = bcx.ins().isub(n, two);
        let fib_ref = module.declare_func_in_func(fib_id, bcx.func);
        let call1 = bcx.ins().call(fib_ref, &[n_minus_1]);
        let r1 = bcx.inst_results(call1)[0];
        let call2 = bcx.ins().call(fib_ref, &[n_minus_2]);
        let r2 = bcx.inst_results(call2)[0];
        let sum = bcx.ins().iadd(r1, r2);
        bcx.ins().return_(&[sum]);

        bcx.finalize(module.target_config());
        module.define_function(fib_id, &mut ctx).expect("define");
        module.clear_context(&mut ctx);
        module.finalize_definitions().expect("finalize");

        let fib_ptr = module.get_finalized_function(fib_id);
        // SAFETY: `fib_id` was declared and defined above with the
        // module's default signature `(i64) -> i64`, the host C calling
        // convention, and `module` keeps the code mapped to the end of
        // the test
        let fib_fn: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fib_ptr) };

        assert_eq!(fib_fn(0), 0);
        assert_eq!(fib_fn(1), 1);
        assert_eq!(fib_fn(10), 55);
        assert_eq!(fib_fn(28), 317811, "fib(28)");
    }
}

mod s1 {
    use crate::jit_backend::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn jit_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let handle = try_compile_int_chunk(cl.proto, false, false)
            .expect("S1 lowerer should accept this chunk");
        // SAFETY: the chunks here are integer arithmetic over locals: no
        // parameters and no helper calls
        unsafe { handle.call_with(&[]) }
    }

    fn interp_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn return_const_int() {
        assert_eq!(jit_int("return 42"), 42);
    }

    #[test]
    fn const_folded_arith() {
        assert_eq!(jit_int("return 1 + 2 + 3"), 6);
    }

    #[test]
    fn locals_add_runtime() {
        assert_eq!(jit_int("local a = 5; local b = 7; return a + b"), 12);
    }

    #[test]
    fn mul_then_add_runtime() {
        assert_eq!(jit_int("local a = 5; return a * a + 1"), 26);
    }

    #[test]
    fn matches_interpreter() {
        for src in [
            "return 0",
            "return 42",
            "return -7",
            "return 1 + 2",
            "local a = 5; local b = 7; return a + b",
            "local a = 5; return a * a + 1",
            "local x = 100; return x - 1",
        ] {
            assert_eq!(jit_int(src), interp_int(src), "src = {src}");
        }
    }

    #[test]
    fn bails_out_on_unsupported_op() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(b"return 'hello'", b"=t").unwrap();
        assert!(try_compile_int_chunk(cl.proto, false, false).is_none());
    }
}

mod s2 {
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
    fn eval_int_chunk_goes_through_jit() {
        assert_eq!(eval_int("return 42"), 42);
    }

    #[test]
    fn eval_local_arith_goes_through_jit() {
        assert_eq!(eval_int("local a = 5; local b = 7; return a + b"), 12);
        assert_eq!(eval_int("local a = 5; return a * a + 1"), 26);
    }

    #[test]
    fn second_call_hits_cached_native() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm
            .load(b"local a = 5; local b = 7; return a + b", b"=t")
            .expect("compile");
        let v1 = vm.call_value(Value::Closure(cl), &[]).unwrap();
        let v2 = vm.call_value(Value::Closure(cl), &[]).unwrap();
        assert!(matches!(v1.first(), Some(Value::Int(12))));
        assert!(matches!(v2.first(), Some(Value::Int(12))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "one compiled Proto"
        );
    }

    #[test]
    fn unsupported_chunk_falls_back_cleanly() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval("return 'hello'").unwrap();
        match v.first() {
            Some(Value::Str(s)) => assert_eq!(s.as_bytes(), b"hello"),
            other => panic!("expected 'hello', got {other:?}"),
        }
    }

    #[test]
    fn args_disable_jit_path() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(b"local a = 5; return a + 1", b"=t").unwrap();
        let v = vm
            .call_value(Value::Closure(cl), &[Value::Int(99)])
            .unwrap();
        assert!(matches!(v.first(), Some(Value::Int(6))));
    }
}

mod s2b {
    //! block-structured lowering with conditional + unconditional
    //! branches. Lt / Le / Eq + Jmp pair into a cranelift `brif`.

    use crate::jit_backend::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn jit_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let handle = try_compile_int_chunk(cl.proto, false, false)
            .expect("S2b lowerer should accept this chunk");
        // SAFETY: the chunks here are integer arithmetic and branches over
        // locals: no parameters and no helper calls
        unsafe { handle.call_with(&[]) }
    }

    fn interp_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    fn parity(src: &str) {
        assert_eq!(jit_int(src), interp_int(src), "src = {src}");
    }

    #[test]
    fn if_lt_true_branch() {
        parity("local x = 2; if x < 3 then return 1 else return 0 end");
    }

    #[test]
    fn if_lt_false_branch() {
        parity("local x = 5; if x < 3 then return 1 else return 0 end");
    }

    #[test]
    fn if_le_boundary() {
        for src in [
            "local x = 3; if x <= 3 then return 1 else return 0 end",
            "local x = 4; if x <= 3 then return 1 else return 0 end",
            "local x = 2; if x <= 3 then return 1 else return 0 end",
        ] {
            parity(src);
        }
    }

    #[test]
    fn if_eq() {
        parity("local x = 5; if x == 5 then return 1 else return 0 end");
        parity("local x = 5; if x == 4 then return 1 else return 0 end");
    }

    #[test]
    fn if_no_else() {
        parity("local x = 5; if x < 3 then return 1 end; return 0");
        parity("local x = 2; if x < 3 then return 1 end; return 0");
    }

    #[test]
    fn nested_if_else() {
        parity(
            "local x = 5; local y = 7; \
             if x < 10 then \
               if y < 5 then return 1 else return 2 end \
             else return 3 end",
        );
        parity(
            "local x = 5; local y = 3; \
             if x < 10 then \
               if y < 5 then return 1 else return 2 end \
             else return 3 end",
        );
        parity(
            "local x = 15; \
             if x < 10 then return 1 else return 3 end",
        );
    }

    #[test]
    fn arith_inside_branches() {
        parity(
            "local a = 5; local b = 7; \
             if a < b then return a * b + 1 else return a - b end",
        );
        parity(
            "local a = 50; local b = 7; \
             if a < b then return a * b + 1 else return a - b end",
        );
    }
}

mod s2c_a {
    //! Protos with `num_params > 0` are JIT-compilable. The
    //! generated `extern "C" fn(...)` takes one i64 per Lua param.
    //! Tested by directly transmuting the raw entry ptr (the
    //! interpreter-side dispatch wire is tested separately).

    use crate::jit_backend::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    /// Keep the `Vm` alive across the body so the GC doesn't reap the
    /// inner closure's Proto. Returning the `Gc<Proto>` past the Vm's
    /// scope is UB — and was the original bug in this test module.
    fn with_inner<F: FnOnce(luna_core::runtime::Gc<luna_core::runtime::function::Proto>)>(
        src: &str,
        f: F,
    ) {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile main");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run main");
        let inner = match r.first() {
            Some(&Value::Closure(inner)) => inner,
            other => panic!("expected the chunk to return one closure, got {other:?}"),
        };
        f(inner.proto);
        drop(vm); // explicit so the borrow checker sees the Vm outlives `f`.
    }

    #[test]
    fn add1_compiles_and_runs() {
        with_inner("local function f(n) return n + 1 end; return f", |proto| {
            let handle =
                try_compile_int_chunk(proto, false, false).expect("S2c.A accepts num_params == 1");
            assert_eq!(handle.num_args(), 1);
            assert!(handle.returns_one());
            for (n, want) in [(41, 42), (0, 1), (-1, 0), (100, 101)] {
                // SAFETY: one integer parameter, and `n + 1` calls no helper
                assert_eq!(unsafe { handle.call_with(&[n]) }, want);
            }
        });
    }

    #[test]
    fn two_param_arith() {
        with_inner(
            "local function f(a, b) return a * b + 1 end; return f",
            |proto| {
                let handle = try_compile_int_chunk(proto, false, false)
                    .expect("S2c.A accepts num_params == 2");
                assert_eq!(handle.num_args(), 2);
                for (a, b, want) in [(3, 4, 13), (5, 6, 31)] {
                    // SAFETY: two integer parameters, and the body calls no helper
                    assert_eq!(unsafe { handle.call_with(&[a, b]) }, want);
                }
            },
        );
    }

    #[test]
    fn param_with_branch() {
        with_inner(
            "local function clip(n) if n < 0 then return 0 end; return n end; return clip",
            |proto| {
                let handle = try_compile_int_chunk(proto, false, false)
                    .expect("S2c.A accepts param + branch");
                assert_eq!(handle.num_args(), 1);
                for (n, want) in [(5, 5), (-5, 0), (0, 0)] {
                    // SAFETY: one integer parameter, and the body calls no helper
                    assert_eq!(unsafe { handle.call_with(&[n]) }, want);
                }
            },
        );
    }

    #[test]
    fn high_arity_bails() {
        with_inner(
            "local function f(a, b, c, d, e) return a + b + c + d + e end; return f",
            |proto| {
                assert!(
                    try_compile_int_chunk(proto, false, false,).is_none(),
                    "5 params is above MAX_JIT_ARITY (4)"
                );
            },
        );
    }
}
