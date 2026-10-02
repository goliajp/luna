//! `if` / `while` conditions built from `and`, `or`, `not`, `~=` and
//! comparisons compile straight to conditional jumps: no comparison result
//! is materialized into a register (`LFalseSkip` / `LoadTrue`) and then
//! tested again.

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::{Heap, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn compile_main(src: &str, v: LuaVersion) -> Vec<Inst> {
    let ast = parse(src.as_bytes(), v).expect("parse");
    let mut heap = Heap::new();
    let proto = compile_chunk(&ast, v, b"=cond", &mut heap).expect("compile");
    proto.code.to_vec()
}

fn count(code: &[Inst], ops: &[Op]) -> usize {
    code.iter().filter(|i| ops.contains(&i.op())).count()
}

fn eval_str(src: &str, v: LuaVersion) -> String {
    let mut vm = Vm::new(v);
    vm.open_base();
    let r = vm
        .eval(src)
        .unwrap_or_else(|e| panic!("{v:?}: {}", vm.error_text(&e)));
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{v:?}: {other:?}"),
    }
}

#[test]
fn and_of_comparisons_jumps_from_both() {
    let src = "local i, n, t = 1, ... while i <= n and t[i] < 7 do i = i + 1 end return i";
    for v in VERSIONS {
        let code = compile_main(src, v);
        assert_eq!(
            count(&code, &[Op::LFalseSkip, Op::LoadTrue, Op::Test]),
            0,
            "{v:?}: {code:?}"
        );
    }
}

#[test]
fn or_not_and_ne_jump_without_a_value() {
    for src in [
        "local a, b = ... if a < b or b < 0 then return 1 end return 0",
        "local a, b = ... if not (a < b) then return 1 end return 0",
        "local a, b = ... if a ~= b then return 1 end return 0",
        "local a, b = ... while not (a == b) and a ~= 0 do a = a - 1 end return a",
    ] {
        for v in VERSIONS {
            let code = compile_main(src, v);
            assert_eq!(
                count(&code, &[Op::LFalseSkip, Op::LoadTrue, Op::Not, Op::Test]),
                0,
                "{v:?} {src}: {code:?}"
            );
        }
    }
}

/// A truthy constant never takes the jump: `while true do` tests nothing.
#[test]
fn constant_conditions_emit_no_test() {
    let src = "local i = 0 while true do i = i + 1 if i > 3 then break end end return i";
    for v in VERSIONS {
        let code = compile_main(src, v);
        assert_eq!(
            count(&code, &[Op::LoadTrue, Op::Test]),
            0,
            "{v:?}: {code:?}"
        );
    }
}

/// Every operand combination takes the branch the condition's value picks,
/// in every dialect; the value side goes through the value compilation of
/// `and` / `or` / `not`, an independent path.
#[test]
fn branches_follow_the_value_of_the_condition() {
    let conds = [
        "a and b",
        "a or b",
        "not a",
        "not (a and b) or c",
        "a and (b or c)",
        "(a or b) and not c",
        "a ~= b",
        "not (a == b) and c",
        "a and true",
        "nil or a",
        "a and nil",
        "false or not b",
        "not not c",
        "(a) and ((b) or not (c))",
        "a and b and c or not a and not b",
    ];
    let mut src = String::from("local vals = { nil, false, true, 0, 'x' } local out = {}\n");
    src.push_str("for i = 1, 5 do for j = 1, 5 do for k = 1, 5 do\n");
    src.push_str("local a, b, c = vals[i], vals[j], vals[k]\n");
    for c in conds {
        src.push_str(&format!(
            "if {c} then out[#out + 1] = '1' else out[#out + 1] = '0' end \
             out[#out + 1] = ({c}) and '1' or '0'\n"
        ));
        src.push_str(&format!(
            "local w = 0 while w < 1 and ({c}) do w = w + 1 end \
             out[#out + 1] = w == 1 and '1' or '0'\n"
        ));
    }
    src.push_str("end end end return table.concat(out)");
    for v in VERSIONS {
        let s = eval_str(&src, v);
        let b = s.as_bytes();
        assert_eq!(b.len() % 3, 0);
        for (n, t) in b.chunks(3).enumerate() {
            assert!(
                t[0] == t[1] && t[2] == t[1],
                "{v:?}: condition {} combination {}: if={} value={} while={}",
                conds[n % conds.len()],
                n / conds.len(),
                t[0] as char,
                t[1] as char,
                t[2] as char
            );
        }
    }
}

#[test]
fn numeric_conditions_follow_their_value() {
    let conds = [
        "a < b and b < c",
        "a <= b or b >= c",
        "not (a < b) and b ~= c",
        "a == 1 or b > 2 and c ~= 3",
        "not (a > 1 or b <= 2)",
    ];
    let mut src = String::from("local out = {}\nfor a = 0, 3 do for b = 0, 3 do for c = 0, 3 do\n");
    for c in conds {
        src.push_str(&format!(
            "if {c} then out[#out + 1] = '1' else out[#out + 1] = '0' end \
             out[#out + 1] = ({c}) and '1' or '0'\n"
        ));
    }
    src.push_str("end end end return table.concat(out)");
    for v in VERSIONS {
        let s = eval_str(&src, v);
        for (n, t) in s.as_bytes().chunks(2).enumerate() {
            assert_eq!(t[0], t[1], "{v:?}: {} at {}", conds[n % conds.len()], n);
        }
    }
}

/// `repeat ... until` conditions jump back from each test like `while`
/// conditions: nothing is materialized into a register and tested again.
#[test]
fn repeat_conditions_jump_without_a_value() {
    for src in [
        "local i, n = 0, ... repeat i = i + 1 until i >= n or i > 9 return i",
        "local i, a = 0, ... repeat i = i + 1 until not (i < 3) and a ~= i return i",
        "local i = 0 repeat local x = i i = i + 1 local f = function() return x end until i > 2 and i ~= 7 return i",
    ] {
        for v in VERSIONS {
            let code = compile_main(src, v);
            assert_eq!(
                count(&code, &[Op::LFalseSkip, Op::LoadTrue, Op::Not, Op::Test]),
                0,
                "{v:?} {src}: {code:?}"
            );
        }
    }
}

/// A `Return0` / `Return1` carries `k` exactly when its function has a
/// register a nested function captures or a to-be-closed variable; the
/// fast return looks for something to close only then, and the closures
/// must still see their variables closed.
#[test]
fn returns_that_must_close_carry_k() {
    let ks = |src: &str, v: LuaVersion| -> Vec<bool> {
        let ast = parse(src.as_bytes(), v).expect("parse");
        let mut heap = Heap::new();
        let proto = compile_chunk(&ast, v, b"=ret", &mut heap).expect("compile");
        let f = &proto.protos[0];
        f.code
            .iter()
            .filter(|i| matches!(i.op(), Op::Return0 | Op::Return1))
            .map(|i| i.k())
            .collect()
    };
    for v in VERSIONS {
        let plain = ks("local function f(a) if a then return 1 end return end", v);
        assert!(
            !plain.is_empty() && plain.iter().all(|&k| !k),
            "{v:?}: {plain:?}"
        );
        let captured = ks(
            "local function f(a) local x = a local g = function() return x end if a then return 1 end end",
            v,
        );
        assert!(
            !captured.is_empty() && captured.iter().all(|&k| k),
            "{v:?}: {captured:?}"
        );
        assert_eq!(
            eval_str(
                "local function mk(a) local x = a local g = function() return x end return g end \
                 local g1, g2 = mk('p'), mk('q') local h = mk('r') return g1() .. g2() .. h()",
                v
            ),
            "pqr",
            "{v:?}"
        );
    }
    let tbc = ks(
        "local function f(a) local c <close> = nil if a then return 1 end end",
        LuaVersion::Lua54,
    );
    assert!(!tbc.is_empty() && tbc.iter().all(|&k| k), "{tbc:?}");
}
