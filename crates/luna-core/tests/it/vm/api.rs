//! PUC `api.lua` exercises the C-API contract (stack ops, call boundary,
//! error propagation, metatable plumbing). luna's analogue is its Rust
//! public surface; these tests pin the same semantic invariants there.

use super::*;

/// Native that pushes its arg count back as the result — analogous to
/// `lua_pushinteger(L, lua_gettop(L))`.
fn api_count_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(nargs as i64)]))
}

/// Native that returns each arg unchanged (multi-return passthrough).
fn api_echo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
    let mut out = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        out.push(vm.nat_arg(fs, nargs, i));
    }
    Ok(vm.nat_return(fs, &out))
}

#[test]
fn api_call_value_zero_args_zero_results() {
    // call_value with empty args, no return: equivalent to `lua_pcall(L, 0, 0)`.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"local x = 1 + 1", b"=chunk").expect("compile");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    assert!(r.is_empty(), "no-return chunk produces zero values: {r:?}");
}

#[test]
fn api_call_value_multi_arg_multi_result() {
    // The chunk consumes vararg, returns 3 values — host gets all three.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm
        .load(b"local a, b = ...; return a + b, a * b, a - b", b"=chunk")
        .expect("compile");
    let r = vm
        .call_value(Value::Closure(cl), &[Value::Int(3), Value::Int(4)])
        .expect("call");
    assert_eq!(r.len(), 3);
    assert!(matches!(r[0], Value::Int(7)), "sum: {:?}", r[0]);
    assert!(matches!(r[1], Value::Int(12)), "prod: {:?}", r[1]);
    assert!(matches!(r[2], Value::Int(-1)), "diff: {:?}", r[2]);
}

#[test]
fn api_native_sees_correct_nargs() {
    // PUC `lua_gettop` from a C function returns the arg count of the
    // call frame. luna's nargs parameter to `NativeFn` is the same.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_count_args);
    vm.set_global("count_args", f).unwrap();
    let v = vm
        .eval("return count_args(), count_args(1), count_args(1,2,3,4,5)")
        .unwrap();
    assert_eq!(v.len(), 3);
    assert!(matches!(v[0], Value::Int(0)));
    assert!(matches!(v[1], Value::Int(1)));
    assert!(matches!(v[2], Value::Int(5)));
}

#[test]
fn api_native_multi_return_passthrough() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_echo);
    vm.set_global("echo", f).unwrap();
    // echo(...) inside a vararg position spreads all results.
    let v = vm.eval("return echo('a','b','c')").unwrap();
    assert_eq!(v.len(), 3);
    for (got, want) in v.iter().zip([b"a", b"b", b"c"].iter()) {
        if let Value::Str(s) = got {
            assert_eq!(s.as_bytes(), *want);
        } else {
            panic!("not a string: {got:?}");
        }
    }
}

#[test]
fn api_globals_round_trip_through_set_and_lua_read() {
    // `Vm::set_global` from Rust must be visible to Lua, and a value Lua
    // stores into a global must be readable via the globals table.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_global("from_host", Value::Int(42)).unwrap();
    let v = vm.eval("return from_host").unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(42)));
    vm.eval("from_lua = 'set by lua'").unwrap();
    let g = vm.globals();
    let key = Value::Str(vm.heap.intern(b"from_lua"));
    let got = g.get(key);
    if let Value::Str(s) = got {
        assert_eq!(s.as_bytes(), b"set by lua");
    } else {
        panic!("expected string, got {got:?}");
    }
}

#[test]
fn api_lua_error_propagates_to_host_with_render() {
    // `error("msg", 0)` raises the bare string; luna's `error_text`
    // renders it the same way PUC's `lua_tostring(L, -1)` would.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"error('boom', 0)", b"=chunk").expect("compile");
    let err = vm
        .call_value(Value::Closure(cl), &[])
        .expect_err("error chunk should fail");
    let text = vm.error_text(&err);
    assert_eq!(text, "boom");
}

#[test]
fn api_call_value_can_catch_internally() {
    // Lua's pcall returns (false, msg) and the host receives a clean Ok.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval(
            "local ok, msg = pcall(function () error('inner') end) \
             return ok, msg",
        )
        .unwrap();
    assert_eq!(v.len(), 2);
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        // 5.5 prepends `chunkname:N:`; we just check the suffix.
        assert!(
            s.as_bytes().ends_with(b"inner"),
            "msg should end with 'inner': {:?}",
            String::from_utf8_lossy(s.as_bytes())
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
}

#[test]
fn api_load_returns_callable_chunk() {
    // `Vm::load` returns a `Gc<LuaClosure>` the host can call repeatedly
    // (the chunk's compiled body is reusable, like `lua_load` produced
    // function on the stack).
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"local n = ...; return n * n", b"=chunk").unwrap();
    let r1 = vm.call_value(Value::Closure(cl), &[Value::Int(5)]).unwrap();
    assert!(matches!(r1[0], Value::Int(25)));
    let r2 = vm.call_value(Value::Closure(cl), &[Value::Int(7)]).unwrap();
    assert!(matches!(r2[0], Value::Int(49)));
}

#[test]
fn api_native_runs_lua_callback_through_call_value() {
    // PUC C-API: `lua_call(L, n, m)` from inside a C function. luna's
    // analogue is `vm.call_value` from within a NativeFn. Native receives
    // a callback function as arg 1, calls it with 10, returns 1 + result.
    fn api_with_callback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
        assert!(nargs >= 1);
        let cb = vm.nat_arg(fs, nargs, 0);
        let r = vm.call_value(cb, &[Value::Int(10)])?;
        let n = if let Value::Int(i) = r[0] {
            Value::Int(i + 1)
        } else {
            Value::Nil
        };
        Ok(vm.nat_return(fs, &[n]))
    }
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_with_callback);
    vm.set_global("with_cb", f).unwrap();
    let v = vm
        .eval("return with_cb(function (x) return x * 3 end)")
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(31)), "got {:?}", v[0]);
}

#[test]
fn api_collect_garbage_returns_freed_count() {
    // PUC `lua_gc(L, LUA_GCCOLLECT, 0)` returns 0 (the previous "freed"
    // count). luna's `collect_garbage` returns the number of objects
    // freed in that pass. We can't pin an exact number across builds,
    // but it should be nonnegative and not panic under repeated calls.
    let mut vm = Vm::new(LuaVersion::Lua55);
    // make a bunch of garbage
    vm.eval("for i = 1, 100 do local t = {i, i} end").unwrap();
    let _ = vm.collect_garbage(); // returns usize ≥ 0 by type
    let _ = vm.collect_garbage(); // second call is a no-op-ish
}
