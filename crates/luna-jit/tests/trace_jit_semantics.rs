//! Operations the trace JIT lowers inline must do what the interpreter
//! does. Each snippet loops long enough for a trace to be recorded and
//! dispatched; it runs with the JIT on and off, the results must match,
//! and the JIT run must have dispatched a trace (so a snippet that stops
//! compiling fails here instead of passing on the interpreter).

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

/// Runs `src` both ways on `version` and returns the shared result.
fn same(version: LuaVersion, src: &str) -> String {
    let (interp, _) = run(version, src, false);
    let (jit, dispatched) = run(version, src, true);
    assert_eq!(jit, interp, "{version:?}: JIT differs from the interpreter");
    assert!(dispatched > 0, "{version:?}: no trace was dispatched");
    interp
}

const INT_DIALECTS: [LuaVersion; 2] = [LuaVersion::Lua54, LuaVersion::Lua55];

/// `//` and `%` round toward minus infinity (lvm.c luaV_idiv/luaV_mod);
/// the trace used truncating `sdiv`/`srem`.
#[test]
fn integer_floor_division_and_modulo() {
    let src = r#"
        local q, r = 0, 0
        for i = 1, 2000 do
          local a = i - 1000
          q = q + a // 7 + a // -7
          r = r + a % 7 + a % -7
        end
        local mi = math.mininteger
        local e, f = 0, 0
        for i = 1, 2000 do e = mi // -1 f = mi % -1 end
        return q .. " " .. r .. " " .. e .. " " .. f"#;
    for v in INT_DIALECTS {
        assert_eq!(same(v, src), "-1715 5 -9223372036854775808 0");
    }
}

/// A zero divisor late in the loop raises the interpreter's error; the
/// trace's `sdiv` trapped and killed the process.
#[test]
fn integer_division_by_zero_raises() {
    for (op, msg) in [
        ("//", "attempt to divide by zero"),
        ("%", "attempt to perform 'n%0'"),
    ] {
        let src = format!(
            "local s = 0 for i = 1, 2000 do local d = 2000 - i s = s + 5 {op} d end return s"
        );
        for v in INT_DIALECTS {
            let out = same(v, &src);
            assert!(out.contains(msg), "{v:?} {op}: {out}");
        }
    }
}

/// Shift counts are not masked: negative shifts the other way, 64 or
/// more gives 0 (luaV_shiftl).
#[test]
fn shifts_by_any_count() {
    let src = r#"
        local s = 0
        for i = 1, 2000 do
          local k = i % 140 - 70
          s = s ~ (1 << k) ~ (-1 >> k) ~ (12345 << -k)
        end
        return tostring(s)"#;
    for v in INT_DIALECTS {
        same(v, src);
    }
}

/// A table read takes the kind its next use implies; the value was used
/// as that kind unchecked, so a table where an integer was expected was
/// added as its pointer bits.
#[test]
fn table_reads_check_the_value_type() {
    const ALL_INT: &[LuaVersion] = &[LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55];
    let cases: [(&str, &[LuaVersion]); 4] = [
        // GetTable in a while loop (5.3 records these; its numeric for
        // loops are not compiled)
        (
            "local t = {} for i = 1, 2000 do t[i] = i end t[1999] = {} \
             local i, s = 1, 0 while i <= 2000 do s = s + t[i] i = i + 1 end return tostring(s)",
            ALL_INT,
        ),
        // GetField
        (
            "local t = {} for i = 1, 2000 do t[i] = {v = i} end t[1999] = {v = {}} \
             local s = 0 for i = 1, 2000 do s = s + t[i].v end return tostring(s)",
            &INT_DIALECTS,
        ),
        // GetTabUp: a global that changes type
        (
            "g = 1 local s = 0 for i = 1, 2000 do if i == 1999 then g = {} end s = s + g end \
             return tostring(s)",
            &INT_DIALECTS,
        ),
        // GetI
        (
            "local s = 0 local t = {} for i = 1, 2000 do t[i] = {i} end t[1999] = {{}} \
             for i = 1, 2000 do s = s + t[i][1] end return tostring(s)",
            &INT_DIALECTS,
        ),
    ];
    for (src, dialects) in cases {
        for &v in dialects {
            let out = same(v, src);
            assert!(out.starts_with("error: "), "{v:?}: {out}");
        }
    }
}

/// `t[k]` with a string key went to the integer-key getter with the
/// string's pointer as the index, and found nothing.
#[test]
fn table_read_with_a_string_key() {
    let src = r#"
        local m = {a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, g = 7, h = 8, i = 9, j = 10}
        local keys = {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"}
        local s = 0
        for n = 1, 300 do for _, k in ipairs(keys) do s = s + m[k] end end
        return tostring(s)"#;
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(same(v, src), "16500");
    }
}

/// `math.<fn>(x)` is replaced by inline code, which is right only while
/// the field holds the library function; assigning another function to
/// it mid-loop kept the inline code running.
#[test]
fn reassigned_math_function_is_called() {
    let src = r#"
        local s = 0
        local m = math
        for i = 1, 3000 do
          s = s + math.min(i, 5)
          if i == 2000 then m.min = m.max end
        end
        return tostring(s)"#;
    for v in INT_DIALECTS {
        assert_eq!(same(v, src), "2510490");
    }
}
