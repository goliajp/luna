//! A local the method JIT reads after a write that only some paths take
//! (`if i == 5 then r = i end`) is still nil on the others: returned, it
//! is nil, and arithmetic on it raises as the interpreter raises, instead
//! of reading the nil's payload as the number 0.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// `(method JIT, trace JIT)`: the method JIT alone and both tiers.
const SETUPS: [(bool, bool); 3] = [(false, false), (true, false), (true, true)];

fn run(version: LuaVersion, src: &str, (method, trace): (bool, bool)) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(method);
    vm.set_trace_jit_enabled(trace);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

#[track_caller]
fn same(src: &str, want: &str) {
    for v in DIALECTS {
        for setup in SETUPS {
            assert_eq!(run(v, src, setup), want, "{v:?} {setup:?}\n{src}");
        }
    }
}

#[test]
fn a_local_written_only_in_a_branch_is_returned_as_nil() {
    same(
        "local function ck(n)
           local r = nil
           local i = 0
           while i < n do i = i + 1 if i == 5 then r = i end end
           return r
         end
         return tostring(ck(40)) .. ' ' .. tostring(ck(1)) .. ' ' .. tostring(ck(40))",
        "5 nil 5",
    );
}

#[test]
fn a_local_written_only_in_a_branch_of_a_for_loop() {
    same(
        "local function ck(n)
           local r
           for i = 1, n do if i == 3 then r = i * 2 end end
           return r
         end
         return tostring(ck(5)) .. ' ' .. tostring(ck(2)) .. ' ' .. tostring(ck(5))",
        "6 nil 6",
    );
}

#[test]
fn arithmetic_on_a_local_still_nil_raises() {
    for v in DIALECTS {
        let src = "local function ck(n)
                     local r
                     for i = 1, n do if i == 3 then r = i end end
                     return r + 1
                   end
                   local a = ck(5)
                   local ok, e = pcall(ck, 2)
                   return tostring(a) .. ' ' .. tostring(ok)";
        let interp = run(v, src, (false, false));
        assert_eq!(interp, "4 false", "{v:?}");
        for setup in SETUPS {
            assert_eq!(run(v, src, setup), interp, "{v:?} {setup:?}");
        }
    }
}
