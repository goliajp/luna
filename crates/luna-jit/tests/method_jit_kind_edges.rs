//! Functions the method JIT sees whose register kinds it cannot lower:
//! they must run as the interpreter runs them (and the compiler must not
//! panic on them).

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const DIALECTS: [LuaVersion; 5] = [
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
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

#[track_caller]
fn same(src: &str) -> Vec<String> {
    DIALECTS
        .iter()
        .map(|&v| {
            let interp = run(v, src, false);
            let jit = run(v, src, true);
            assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
            interp
        })
        .collect()
}

/// The register that held the returned table in one branch takes an
/// arithmetic result in the other.
#[test]
fn arithmetic_into_a_table_register() {
    for op in ["+", "-", "*", "/"] {
        let src = format!(
            "local function f(n)
               if n == 0 then return {{1, 1}} end
               local x = n {op} n
               return {{x, x}}
             end
             local r = ''
             for i = 1, 3 do r = r .. tostring(f(i)[1]) .. ' ' end
             return r .. tostring(#f(0))"
        );
        same(&src);
    }
}

/// A math library call on a register that holds a table raises in the
/// interpreter.
#[test]
fn math_call_on_a_table() {
    let src = "local function f(n)
                 local t = {}
                 if n == 0 then return math.floor(t) end
                 return n
               end
               f(1) f(2)
               local ok, e = pcall(f, 0)
               return tostring(ok) .. ' ' .. tostring(e)";
    for got in same(src) {
        assert!(got.starts_with("false "), "{got}");
    }
}

/// A step of `math.mininteger` (a constant, so the method JIT knows it):
/// the loop runs once.
#[test]
fn mininteger_step() {
    let src = "local function f(n)
                 local c = 0
                 for i = n, -n, 0x8000000000000000 do c = c + 1 end
                 return c
               end
               return tostring(f(5)) .. ' ' .. tostring(f(5))";
    for (v, got) in DIALECTS.iter().zip(same(src)) {
        if *v >= LuaVersion::Lua54 {
            assert_eq!(got, "1 1", "{v:?}");
        }
    }
}
