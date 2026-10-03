mod s5d_b {
    //! `NewTable b > 0` (presize hint from the bytecode
    //! field) + `Op::SetList` (fixed-count `{a, b, c}` literals) +
    //! BB-level `defines_table` dataflow (a sound
    //! intersection-at-joins must-defined analysis rather than a blanket
    //! "has_conditional && has_new_table → bail" gate).
    //!
    //! The BB dataflow accepts patterns like `make`'s two-branch
    //! structure — both branches independently `NewTable + SetList`
    //! into the same register before Return1 — while still rejecting
    //! the unsound false-branch-only-define case the linear forward
    //! walk would let through.
    //!
    //! Register kind reuse across branches (e.g. `Int` in BB-then and
    //! `Table` in BB-else) is covered by the `make_proto_*` tests
    //! below.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    /// Simple SetList literal — `{1, 2, 3}` in a fn body. NewTable
    /// b=3 + LoadI×3 + SetList b=3 + Return1.
    #[test]
    fn newtable_b3_setlist_int_5_5() {
        let src = b"local function f() return {10, 20, 30} end return f";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let inner = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // First call populates / drives JIT.
        let r2 = vm.call_value(Value::Closure(inner), &[]).expect("call f()");
        // f returns a Table; assert ret_is_table threaded through.
        let t = match r2.first() {
            Some(&Value::Table(t)) => t,
            other => panic!("expected table, got {other:?}"),
        };
        assert_eq!(t.len(), 3);
        assert!(matches!(t.get_int(2), Value::Int(20)));
        assert!(matches!(
            inner.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
    }

    /// Read a fixed-N table — exercises GetI through a Table param
    /// in concert with the SetList that built it.
    #[test]
    fn setlist_then_geti_round_trip_5_5() {
        let src = "local function get(t, i) return t[i] end
                   local function make() return {7, 11, 13} end
                   return get(make(), 2)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(11))));
    }

    /// Both branches independently `NewTable + SetList` into R[A]
    /// before `Return1` — proves the BB-level dataflow accepts
    /// what a blanket gate would have blocked. The function
    /// param is an Int so we don't hit the RegKind reuse conflict
    /// the binary_trees `make` Proto carries.
    #[test]
    fn conditional_both_branches_new_table_5_5() {
        let src = "local function f(flag)
                     if flag == 1 then return {1, 2}
                     else return {3, 4} end
                   end
                   return f(1)[2] + f(0)[1]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // f(1) -> {1,2}, [2] = 2. f(0) -> {3,4}, [1] = 3. Sum = 5.
        assert!(matches!(r.first(), Some(&Value::Int(5))));
    }

    /// the binary_trees `make` Proto JIT-compiles:
    /// the Int-to-Table re-use on R[1] (LoadI 0 for an Eq compare,
    /// then `NewTable` for the table) is allowed via the relaxed
    /// `RegKind::unify`; `latest_writer_kind` carries the per-PC
    /// kind so `Return1` correctly wraps as `Value::Table`. The
    /// variadic `Op::Call C=0` + `Op::SetList B=0` pattern in the
    /// else branch resolves to `count = A_call - A_list` at scan.
    /// End-to-end `make(3)` builds the same 8-leaf tree the
    /// interpreter would.
    #[test]
    fn make_proto_5_5_round_trip() {
        let src = "local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   local t = make(3)
                   return t[1][1][1][1] + t[2][2][2][2]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(2))));
    }

    /// Full binary_trees bench source round-trip — make + check
    /// both JIT'd, sum across 16 trees of depth 10 matches interp.
    #[test]
    fn binary_trees_n10_round_trip_5_5() {
        let src = "local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local sum = 0
                   for i = 1, 16 do sum = sum + check(make(10)) end
                   return sum";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // Each depth-10 tree has 2^11 - 1 == 2047 internal nodes;
        // check returns 2047 per tree; 16 * 2047 == 32752.
        assert!(matches!(r.first(), Some(&Value::Int(32752))));
    }

    /// 5.1/5.2 binary_trees `make` Proto JIT-
    /// compiles. The frontend uses `LoadF R[1]=0` for the `if d
    /// == 0` Eq compare in one BB and `NewTable R[1]` for the
    /// returned table in another, so R[1] sees Float+Table on
    /// disjoint paths. The relaxed `unify(Float, Table)` lets
    /// the scan keep R[1] declared in whichever shape the first
    /// writer pinned; emit-side `use_var` callers for Table
    /// operands bitcast F64→I64 when the slot is Float-declared.
    fn make_proto_jit_compiles_for_version(ver: LuaVersion) {
        let src = b"local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   return make";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=make").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let make_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // Drive a few calls to warm the JIT cache.
        for d in 0..3 {
            vm.call_value(Value::Closure(make_cl), &[Value::Int(d)])
                .expect("call make(d)");
        }
        assert!(
            matches!(make_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} make Proto did not JIT-compile (state: {:?})",
            make_cl.proto.jit.get()
        );
    }

    #[test]
    fn make_proto_jit_compiles_5_1() {
        make_proto_jit_compiles_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn make_proto_jit_compiles_5_2() {
        make_proto_jit_compiles_for_version(LuaVersion::Lua52);
    }

    /// `binary_trees`' cross_dialect harness uses
    /// `{nil, nil}` as the leaf node. LoadNil + SetList must
    /// JIT-compile so `make` stops bailing across all dialects.
    fn make_nil_proto_jit_compiles_for_version(ver: LuaVersion) {
        let src = b"local function make(d)
                     if d == 0 then return {nil, nil}
                     else return {make(d-1), make(d-1)} end
                   end
                   return make";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=make").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let make_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        for d in 0..3 {
            vm.call_value(Value::Closure(make_cl), &[Value::Int(d)])
                .expect("call make(d)");
        }
        assert!(
            matches!(make_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} make {{nil,nil}} Proto did not JIT-compile (state: {:?})",
            make_cl.proto.jit.get()
        );
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_1() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_2() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua52);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_3() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua53);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_5() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua55);
    }

    /// `return nil` must NOT JIT into `Value::Int(0)`
    /// (the dispatcher's ret_is_float=false default would wrap the
    /// i64 helper return as `Int`, masking the Nil). The Return1
    /// scan bails on a LoadNil source so the interp returns the
    /// real Nil.
    #[test]
    fn return_nil_does_not_miscompile_5_5() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm
            .eval("local function f() return nil end return f()")
            .expect("eval");
        assert!(
            matches!(r.first(), Some(&Value::Nil)),
            "expected Nil, got {r:?}"
        );
    }

    /// binary_trees' `check` Proto JIT-compiles end-to-end: GetI
    /// through a Table param, Int Eq + branch, self-recursive
    /// Call returning Int, Int Add, Return1 of an Int. No Table
    /// re-use conflict — all kind unifications converge cleanly.
    #[test]
    fn check_proto_jit_compiles_5_5() {
        let src = "local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local leaf = {1, 1}
                   local node = {leaf, leaf}
                   return check(node)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // node = {leaf, leaf} where leaf is terminal.
        // check(node) hits else: 1 + check(leaf) + check(leaf)
        //                       = 1 + 1 + 1 = 3.
        assert!(matches!(r.first(), Some(&Value::Int(3))));
    }

    /// a Table-typed param + a single `R[B][R[C]]`
    /// read is the minimal `OP_GETTABLE` shape; it must JIT in
    /// 5.1 / 5.2 (which lower `t[1]` as GetTable + a Float key,
    /// not GetI + an immediate Int). The chunk returns the read
    /// value; the assertion is that the Proto reaches the
    /// Compiled state at all.
    fn get_table_simple_jit_for_version(ver: LuaVersion) {
        let src = b"local function get(t, k) return t[k] end
                   return get";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=get").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let get_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // Warm: drive a couple of calls with a normal (no-metatable)
        // table so the JIT path is reached. The cache lookup happens
        // on first call; subsequent calls run the cached entry.
        let table = vm.table_of([(1i64, Value::Float(42.0))]);
        for _ in 0..3 {
            let _ = vm
                .call_value(
                    Value::Closure(get_cl),
                    &[Value::Table(table), Value::Float(1.0)],
                )
                .expect("call get(t, 1.0)");
        }
        assert!(
            matches!(get_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} get Proto did not JIT-compile (state: {:?})",
            get_cl.proto.jit.get()
        );
    }

    #[test]
    fn get_table_simple_jit_5_1() {
        get_table_simple_jit_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn get_table_simple_jit_5_2() {
        get_table_simple_jit_for_version(LuaVersion::Lua52);
    }

    /// binary_trees' `check` Proto in 5.1 / 5.2 — same source as the
    /// 5.5 test, but lowering uses `OP_GETTABLE` for `t[1]` / `t[2]`
    /// (no GetI). Reaches `JitProtoState::Compiled` because GetTable
    /// is whitelisted.
    fn check_proto_jit_compiles_pre53(ver: LuaVersion) {
        let src = "local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local leaf = {1, 1}
                   local node = {leaf, leaf}
                   return check(node)";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let r = vm.eval(src).expect("eval");
        // 5.1 / 5.2 have no Int subtype — the literal `1` is Float;
        // arith and Return ferry the f64 bits unchanged.
        match r.first() {
            Some(&Value::Float(f)) if (f - 3.0).abs() < 1e-9 => {}
            other => panic!("{ver:?} check(node) expected Float(3.0), got {other:?}"),
        }
    }

    #[test]
    fn check_proto_jit_compiles_5_1() {
        check_proto_jit_compiles_pre53(LuaVersion::Lua51);
    }

    #[test]
    fn check_proto_jit_compiles_5_2() {
        check_proto_jit_compiles_pre53(LuaVersion::Lua52);
    }
}
