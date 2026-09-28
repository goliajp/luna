//! `t[k]` with a float key in method-JIT code. The fast path turns an
//! integral float key into an array index; it used Cranelift's trapping
//! float-to-int conversion, so a NaN, infinite or out-of-range key killed
//! the process with SIGILL instead of reading nil. 2^63 also has to stay
//! apart from `math.maxinteger`: it saturates to i64::MAX and converts back
//! to 2^63, so only the range check tells them apart.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run(version: LuaVersion, src: &str, jit: bool) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {e}"),
    }
}

fn same(version: LuaVersion, src: &str, expected: &str) {
    assert_eq!(
        run(version, src, false),
        expected,
        "{version:?} interpreter"
    );
    assert_eq!(run(version, src, true), expected, "{version:?} JIT");
}

#[test]
fn nan_key_read_in_the_main_chunk_is_nil() {
    for v in ALL {
        same(v, "local t = {} return tostring(t[0/0])", "nil");
    }
}

#[test]
fn non_integral_and_out_of_range_float_keys_read_through_the_hash() {
    let src = r#"
        local t = {10, 20, 30}
        t[2^63] = "f"
        if math.maxinteger then t[math.maxinteger] = "i" end
        local function get(k) return t[k] end
        local keys = {0/0, -(0/0), 1/0, -1/0, 2^63, -2^63, 1e300, 1.5, 2.0, 3.0}
        local out
        for _ = 1, 200 do
          out = {}
          for i, k in ipairs(keys) do out[i] = tostring(get(k)) end
        end
        return table.concat(out, ",")"#;
    for v in ALL {
        same(v, src, "nil,nil,nil,nil,f,nil,nil,nil,20,30");
    }
}

/// The read's result register is typed from how the chunk uses it; a value
/// of another type (nil for a missing key, a string) must not come back as
/// that type's zero. The compiled read checked nothing and returned the
/// raw payload, so `return t[0/0]` gave 0. The chunks return the value
/// itself: a call to a library function would keep the chunk out of the
/// method JIT, which compiles only self-recursive calls.
#[test]
fn a_read_of_another_type_is_not_reinterpreted() {
    fn raw(version: LuaVersion, src: &str, jit: bool) -> String {
        let mut vm = luna_jit::new_with_jit(version);
        vm.set_jit_enabled(jit);
        vm.set_trace_jit_enabled(jit);
        match vm.eval(src).expect("eval").first() {
            Some(Value::Nil) | None => "nil".into(),
            Some(Value::Int(i)) => format!("int {i}"),
            Some(Value::Float(f)) => format!("float {f}"),
            Some(Value::Str(s)) => format!("str {}", String::from_utf8_lossy(s.as_bytes())),
            Some(other) => format!("{other:?}"),
        }
    }
    let cases = [
        ("local t = {} return t[0/0]", "nil"),
        ("local t = {1, 2} return t[5]", "nil"),
        ("local t = {1, 'x'} return t[2]", "str x"),
        ("local t = {1, 2} local k = 2 return t[k]", "int 2"),
    ];
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for (src, want) in cases {
            assert_eq!(raw(v, src, false), want, "{v:?} interpreter: {src}");
            assert_eq!(raw(v, src, true), want, "{v:?} JIT: {src}");
        }
    }
}
