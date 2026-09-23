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
