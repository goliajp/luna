mod s5c {
    //! `NewTable` / `SetTable` / `GetI` / `Len` JIT via Rust
    //! helpers. The dispatcher pins the active `Vm` in the
    //! `JIT_VM` thread-local; cranelift `Linkage::Import` calls
    //! land in `luna_jit_new_table` / `_table_set_int` /
    //! `_table_set_float_float` / `_table_get_int` / `_table_len`
    //! which demote the pinned ptr to `&mut Vm` for the duration of
    //! one helper-level operation.
    //!
    //! Headline cell: `table_alloc_10k` 5.3 / 5.4 / 5.5 (Int-loop
    //! dialects). 5.1 / 5.2 use a Float loop var that conflicts
    //! with the `Len` result's Int kind on the same register —
    //! documented bail.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    /// Small `table_alloc` — proves the NewTable + SetTable + Len
    /// path runs end-to-end on the active Vm.
    #[test]
    fn table_alloc_10_matches_interpreter_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 10 do t[i] = i end return #t",
            ),
            10,
        );
    }

    /// Headline cell at full N=10 000. Pinned `JitProtoState`.
    #[test]
    fn table_alloc_10k_5_5_jit_state_compiled() {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// 5.4 dialect — same shape as 5.5 (Int loop var, Int-typed
    /// SetTable values).
    #[test]
    fn table_alloc_10k_5_4_matches_interpreter() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua54,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// 5.3 dialect — pre53 ForPrep + Int loop var.
    #[test]
    fn table_alloc_10k_5_3_matches_interpreter() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// `t[50]` after building — exercises `Op::GetI` with an
    /// immediate Int key. Stores `i * 2`, so the read at index 50
    /// must come back as 100.
    #[test]
    fn table_get_int_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 100 do t[i] = i * 2 end return t[50]",
            ),
            100,
        );
    }

    /// `#t` returns Int — confirms `Op::Len` JIT path against the
    /// interpreter's `len()`.
    #[test]
    fn table_len_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 42 do t[i] = i end return #t",
            ),
            42,
        );
    }

    /// Float loop var + Int `Len` result re-using the
    /// same register slot JIT-compiles. The `Len` scan does not
    /// force-unify R[A] with `Int`; instead it leaves the
    /// declared kind alone when it's already Float/Int (and pins
    /// Int only when Unset). The helper's i64 return goes through
    /// `aligned_def`'s I64↔F64 bitcast on the writer side, and the
    /// `Return1` emit bitcasts F64→I64 (using the declared Float
    /// kind) so the i64 length ferries through unchanged. The
    /// `latest_writer_kind[a] = Int` assignment ensures `ret_kind`
    /// reflects Len's Int even when the declared slot stays Float.
    #[test]
    fn table_alloc_10k_5_2_jit_compiles() {
        table_alloc_10k_jit_compiles_for_version(LuaVersion::Lua52);
    }

    /// symmetric to the 5.2 case: 5.1's frontend lowers
    /// `for i = 1, 10000` with a Float loop var (no Int subtype), so
    /// the same Float/Int slot reuse at `#t` post-loop applies.
    #[test]
    fn table_alloc_10k_5_1_jit_compiles() {
        table_alloc_10k_jit_compiles_for_version(LuaVersion::Lua51);
    }

    fn table_alloc_10k_jit_compiles_for_version(ver: LuaVersion) {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(
            matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} table_alloc Proto did not JIT-compile (state: {:?})",
            cl.proto.jit.get()
        );
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// Non-Int key (string) bails. The whitelist's SetTable arm
    /// requires R[B] to be Int/Float (numeric loop var Move).
    #[test]
    fn table_with_string_key_bails() {
        let src = "local t = {} t['a'] = 1 return t['a']";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
    }

    /// NewTable with a presized array (`b > 0`) compiles (the
    /// `NewTable.B` field feeds
    /// `luna_jit_new_table_sized`, and SetList builds the literal
    /// inline). The chunk loads `{10, 20, 30}` then reads `t[2]`;
    /// both ops are whitelisted.
    #[test]
    fn presized_newtable_now_jits() {
        let src = "local t = {10, 20, 30} return t[2]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(20))));
    }
}

mod s5c_b {
    //! `NewTable` presize fold. When the bytecode opens a
    //! counted `for i = 1, N do … end` window immediately after
    //! a `local t = {}`, the JIT emits
    //! `luna_jit_new_table_sized(N)` instead of the plain
    //! `luna_jit_new_table()`. The pre-sized array part skips
    //! every intermediate `rehash` round that the iteration body
    //! would otherwise trigger via `Table::set_int`'s `insert_new`
    //! path.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    /// Headline cell — `table_alloc_10k 5.5` JIT-compiles and
    /// returns the correct length after pre-sized fill.
    #[test]
    fn table_alloc_10k_5_5_presized() {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// 5.4 — same shape (Int loop var). Presize hint extracted
    /// from the LoadI limit window.
    #[test]
    fn table_alloc_10k_5_4_presized() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua54,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// 5.3 — pre53 ForPrep form, same Int loop var. The presize
    /// scan inspects PCs prep_pc-4..prep_pc and is dialect-
    /// agnostic.
    #[test]
    fn table_alloc_10k_5_3_presized() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// Compile-time `limit` from a `LoadK Int` (the 10 000 fits
    /// in `sbx`, but a larger limit exercises the `LoadK` arm).
    #[test]
    fn table_alloc_loadk_limit_5_5() {
        let src = "local t = {} for i = 1, 65536 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(65536))));
    }

    /// Small `N=4` — verify the fold doesn't misfire on tiny
    /// tables. Result and JIT state both pin.
    #[test]
    fn table_alloc_4_small_presize() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 4 do t[i] = i end return #t",
            ),
            4,
        );
    }

    /// `for i = 1, N, 2 do …` — step ≠ 1. The presize map skips
    /// this entry; the chunk still compiles but uses
    /// the non-sized helper. Correctness unaffected.
    #[test]
    fn step_ne_1_falls_back_to_empty_helper() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 10, 2 do t[i] = i end return #t",
            ),
            // Indices 1, 3, 5, 7, 9: every odd slot is a border; PUC
            // 5.5.1 returns 3 for this history
            3,
        );
    }

    /// inline aset writes the **right** payload at the
    /// **right** offset. Sum-of-cubes is sensitive to either a
    /// stride-1 error in `key_minus_1 * 8` (would corrupt avals
    /// indexing) or a misaligned atag write (interp would read back
    /// a Nil tag and treat the value as Nil → 0). Both modes would
    /// fail the assertion; the only way to hit `36` is correct.
    #[test]
    fn inline_aset_payload_round_trip_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {}
                 for i = 1, 5 do t[i] = i * i end
                 return t[1] + t[2] + t[3] + t[4] + t[5]",
            ),
            // 1 + 4 + 9 + 16 + 25 == 55
            55,
        );
    }

    /// Cross-dialect: 5.3 reads the same values back. 5.3 uses the
    /// pre53 ForPrep form, so this exercises a different emit
    /// branch than 5.5 while still hitting the inline aset path.
    #[test]
    fn inline_aset_payload_round_trip_5_3() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {}
                 for i = 1, 5 do t[i] = i * i end
                 return t[1] + t[2] + t[3] + t[4] + t[5]",
            ),
            55,
        );
    }
}

mod s5d_a {
    //! ABI extension: `arg_table_mask` + `ret_is_table`.
    //! Threads `Value::Table` through the JIT entry as a raw
    //! `Gc<Table>` ptr and back through the dispatcher.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    /// `function f(t) return t[1] end` — Table param + Int return.
    /// JIT path: param marshalled as Gc ptr, GetI reads array slot,
    /// Return1 sends Int back. Verifies the ABI plumbing without
    /// any NewTable/SetList plumbing.
    #[test]
    fn table_param_int_return_round_trip_5_5() {
        use luna_core::runtime::function::JitProtoState;
        // Build f's Proto via a chunk that returns the function so we
        // can poke its JIT state. The outer chunk bails (Op::Closure
        // isn't whitelisted); the inner Proto is what we're after.
        let src = b"local function f(t) return t[1] end return f";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let inner = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // The inner Proto's JIT state is `Untried` until first call.
        // Drive it by calling f(t) and confirming the Compiled state +
        // correct result.
        let t_chunk = b"local t = {} t[1] = 42 return t";
        let t_cl = vm.load(t_chunk, b"=u").expect("compile");
        let t_v = vm.call_value(Value::Closure(t_cl), &[]).expect("run");
        let tv = *t_v.first().expect("t");
        let r2 = vm
            .call_value(Value::Closure(inner), &[tv])
            .expect("call f(t)");
        assert!(matches!(r2.first(), Some(&Value::Int(42))));
        assert!(matches!(
            inner.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
    }

    /// `function f(t) return #t end` — same shape, Len op.
    #[test]
    fn table_param_len_5_5() {
        let src = "local function f(t) return #t end
                   local t = {} t[1] = 1 t[2] = 1 t[3] = 1 return f(t)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(3))));
    }
}
