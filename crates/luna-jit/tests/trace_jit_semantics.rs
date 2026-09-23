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
        local g, h = 0, 0
        for i = 1, 2000 do local x = mi + i - 1 g = x // 7 h = x % 7 end
        return q .. " " .. r .. " " .. e .. " " .. f .. " " .. g .. " " .. h"#;
    for v in INT_DIALECTS {
        assert_eq!(
            same(v, src),
            "-1715 5 -9223372036854775808 0 -1317624576693539116 3"
        );
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
/// string's pointer as the index, and found nothing. Such a read is now
/// left to the interpreter (the trace is not dispatched), so this only
/// compares results.
#[test]
fn table_read_with_a_string_key() {
    let src = r#"
        local m = {a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, g = 7, h = 8, i = 9, j = 10}
        local keys = {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"}
        local s = 0
        for n = 1, 300 do for _, k in ipairs(keys) do s = s + m[k] end end
        return tostring(s)"#;
    for v in INT_DIALECTS {
        let (interp, _) = run(v, src, false);
        let (jit, _) = run(v, src, true);
        assert_eq!(interp, "16500");
        assert_eq!(jit, interp, "{v:?}");
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

const SHOW: &str = r#"
local function show(v) return string.format("%.17g", v) .. ":" .. tostring(math.type(v)) end
"#;

/// 5.4+ `floor` / `ceil` of a float compile: an integer when the result
/// fits, and a trace exit to the interpreter (which returns the float)
/// when it does not. Expected values are PUC 5.4.9 / 5.5.1's.
#[test]
fn floor_and_ceil_of_floats() {
    let fits = "local c, x = 0, 0.5 \
                for i = 1, 3000 do x = x + 1.0 c = c + math.ceil(x) - math.floor(x) end \
                return tostring(c) .. ' ' .. math.type(math.floor(x))";
    let huge = format!(
        "{SHOW} local big, y = 0, 0.0 \
         for i = 1, 3000 do y = y + 1.0 local z = y if i == 2500 then z = 1e300 end \
         big = math.floor(z) end return show(big)"
    );
    let nan = format!(
        "{SHOW} local r, y = 0, 0.0 \
         for i = 1, 3000 do y = y + 1.0 if i == 3000 then y = 0/0 end r = math.ceil(y) end \
         return show(r)"
    );
    for v in INT_DIALECTS {
        assert_eq!(same(v, fits), "3000 integer");
        assert_eq!(same(v, &huge), "3000:integer");
        let out = same(v, &nan);
        assert!(out.ends_with(":float"), "{v:?}: {out}");
    }
}

/// `max` / `min` of an integer and a float return the winning argument
/// unconverted, compared exactly (2^53 + 1 is not below 2^53 as floats
/// would say).
#[test]
fn max_and_min_of_mixed_kinds() {
    let cases = [
        (
            format!(
                "{SHOW} local m1, m2 = 0, 0 \
                 for i = 1, 3000 do m1 = math.max(i, 1500.5) m2 = math.min(i, 2000.25) end \
                 return show(m1) .. ' ' .. show(m2)"
            ),
            "3000:integer 2000.25:float",
        ),
        (
            format!(
                "{SHOW} local m, f = 0, 0.0 \
                 for i = 1, 3000 do f = f + 1.0 m = math.max(f, i) end return show(m)"
            ),
            "3000:float",
        ),
        (
            format!(
                "{SHOW} local m = 0 \
                 for i = 1, 3000 do m = math.min(9007199254740992.0, i + 9007199254740990) end \
                 return show(m)"
            ),
            "9007199254740992:float",
        ),
        (
            "local lo = 0 \
             for i = 1, 3000 do lo = math.max(2^53, 9007199254740993) end \
             return tostring(lo) .. ':' .. math.type(lo)"
                .to_string(),
            "9007199254740993:integer",
        ),
        (
            format!(
                "{SHOW} local z, nz = 0, -0.0 \
                 for i = 1, 3000 do z = math.max(0, nz) end return show(z)"
            ),
            "0:integer",
        ),
    ];
    for (src, expected) in &cases {
        for v in INT_DIALECTS {
            assert_eq!(same(v, src), *expected, "{v:?}: {src}");
        }
    }
}

/// 5.4+ `atan(y)` folds as `atan2(y, 1)`, rounded as the interpreter
/// (and PUC on the same libm) does.
#[test]
fn atan_folds_as_atan2() {
    let src = "local a, t = 0, 0.00012682450675524315 \
               for i = 1, 3000 do a = math.atan(t) end return string.format('%.17g', a)";
    for v in INT_DIALECTS {
        same(v, src);
    }
}

/// A `pairs` loop compiled over string keys ran its body again for an
/// integer key (the back-edge only asked for an integer tag), storing
/// the key tagged as a string; `table.sort` then read it as a string
/// pointer and crashed. The back-edge now requires the tags the body was
/// compiled for. Recording starts early, as it would in a longer run.
#[test]
fn pairs_loop_meeting_other_key_kinds() {
    let src = r#"
        local function keys(t)
          local ks = {}
          for k in pairs(t) do ks[#ks + 1] = k end
          table.sort(ks, function(a, b) return tostring(a) < tostring(b) end)
          local out = {}
          for i = 1, #ks do out[i] = tostring(ks[i]) .. ":" .. type(ks[i]) end
          return table.concat(out, ",")
        end
        local vals = {}
        local function vals_of(t)
          local s = 0
          for _, v in pairs(t) do s = s + (type(v) == "number" and v or 100) end
          return s
        end
        local tabs = {{a = 1, b = 2}, {a = 1, b = 2, c = 3}, {x = 1}, {y = 1, z = 2},
                      {[3] = true, [4] = true}, {[5] = true}, {q = 1}, {p = "s", r = 2}}
        local res = {}
        for _, t in ipairs(tabs) do res[#res + 1] = keys(t) .. "/" .. vals_of(t) end
        return table.concat(res, " ")"#;
    for v in INT_DIALECTS {
        let (interp, _) = run(v, src, false);
        let mut vm = luna_jit::new_with_jit(v);
        vm.jit.trace_hot_threshold = 1;
        vm.jit.call_hot_threshold = 1;
        let jit = match vm.eval(src).expect("eval").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        };
        assert_eq!(jit, interp, "{v:?}");
        assert!(
            vm.trace_dispatched_count() > 0,
            "{v:?}: no trace dispatched"
        );
    }
}

/// A numeric string in arithmetic is coerced; the trace added the
/// string's pointer as an integer.
#[test]
fn string_operand_in_arithmetic() {
    let src = r#"
        local s, x = "10", 0
        for i = 1, 200 do x = i + s end
        local t, y = "7", 0
        for i = 1, 200 do y = (i % t) + (i // t) end
        return tostring(x) .. " " .. tostring(y)"#;
    for v in INT_DIALECTS {
        let (interp, _) = run(v, src, false);
        let mut vm = luna_jit::new_with_jit(v);
        vm.jit.trace_hot_threshold = 1;
        let jit = match vm.eval(src).expect("eval").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        };
        assert!(interp.starts_with("210 "), "{v:?}: {interp}");
        assert_eq!(jit, interp, "{v:?}");
    }
}

/// An outer numeric for around a `while` loop: the while loop's trace
/// ran natively while the outer loop's side trace was being recorded,
/// the recording missed it and closed as a two-op "loop" returning its
/// own head, and the dispatcher re-entered that trace forever.
#[test]
fn recording_does_not_span_a_compiled_trace() {
    let src = r#"
        local t = {} for i = 1, 1000 do t[i] = i end
        local s = 0
        for r = 1, 20 do local i = 1 while i <= 1000 do s = s + t[i] i = i + 1 end end
        return tostring(s)"#;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for v in INT_DIALECTS {
            let (jit, dispatched) = run(v, src, true);
            tx.send((v, jit, dispatched)).expect("send");
        }
    });
    for _ in INT_DIALECTS {
        let (v, jit, dispatched) = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("the JIT run did not finish in 60 s");
        assert_eq!(jit, "10010000", "{v:?}");
        assert!(dispatched > 0, "{v:?}: no trace dispatched");
    }
}

/// A hot function whose trace does not compile was recorded and
/// compiled again on every later call (299,936 failed compiles for
/// 300,000 calls, 19x the interpreter's time). A head is dropped after a
/// few failures.
#[test]
fn a_failing_trace_is_not_recompiled_on_every_call() {
    let src = r#"
        local function f(x) local y = x + "1" return y end
        local s = 0
        for i = 1, 20000 do s = s + f(i) end
        return tostring(s)"#;
    for v in INT_DIALECTS {
        let mut vm = luna_jit::new_with_jit(v);
        let out = match vm.eval(src).expect("eval").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        };
        assert_eq!(out, "200030000", "{v:?}");
        let failed = vm.jit.counters.compile_failed;
        assert!(
            failed > 0,
            "{v:?}: the trace was expected to fail to compile"
        );
        assert!(failed <= 10, "{v:?}: {failed} failed compiles");
    }
}

/// A trace that cannot change `math.min` itself checks it once, before
/// its loop; a reassignment between two runs of the loop must still be
/// seen.
#[test]
fn math_function_reassigned_between_dispatches() {
    let src = r#"
        local function run()
          local s = 0
          for i = 1, 300 do s = s + math.min(i, 5) end
          return s
        end
        local a = run()
        math.min = math.max
        local b = run()
        return a .. " " .. b"#;
    for v in INT_DIALECTS {
        assert_eq!(same(v, src), "1490 45160", "{v:?}");
    }
}

/// Run `src` on a Vm that records traces after one back-edge or call and
/// compare with the interpreter.
fn same_hot(v: LuaVersion, src: &str) -> String {
    let (interp, _) = run(v, src, false);
    let mut vm = luna_jit::new_with_jit(v);
    vm.jit.trace_hot_threshold = 1;
    vm.jit.call_hot_threshold = 1;
    let jit = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    assert_eq!(jit, interp, "{v:?}");
    assert!(
        vm.trace_dispatched_count() > 0,
        "{v:?}: no trace dispatched"
    );
    interp
}

/// A generic-for over a native iterator ends with the value slot holding
/// what the iterator's last call left (nil), but the trace restored it
/// under its entry tag: a string with a null pointer (panic "gc pointer
/// must be non-null" on the string_pattern probe's gmatch loop).
#[test]
fn generic_for_exit_restores_the_loop_variables_as_nil() {
    let src = r#"
        local out = {}
        for rep = 1, 3 do
          local r = {}
          for a, b in string.gmatch("abcdef", "()(.)") do r[#r + 1] = a .. b end
          out[#out + 1] = table.concat(r, ",")
        end
        return table.concat(out, " | ")"#;
    for v in INT_DIALECTS {
        assert_eq!(
            same_hot(v, src),
            "1a,2b,3c,4d,5e,6f | 1a,2b,3c,4d,5e,6f | 1a,2b,3c,4d,5e,6f"
        );
    }
}

/// When the next value has another kind than the loop body was compiled
/// for, the trace leaves at the TForLoop; the dispatcher then re-tagged
/// the helper's new value with the old kind.
#[test]
fn generic_for_value_changing_kind() {
    let src = r#"
        local r = {}
        for k, v in pairs({10, 20, 30, "x", "y", 60, 70, 80}) do r[#r + 1] = v end
        local out = {}
        for i = 1, #r do out[i] = type(r[i]) .. ":" .. tostring(r[i]) end
        return table.concat(out, ",")"#;
    for v in INT_DIALECTS {
        assert_eq!(
            same_hot(v, src),
            "number:10,number:20,number:30,string:x,string:y,number:60,number:70,number:80"
        );
    }
}
