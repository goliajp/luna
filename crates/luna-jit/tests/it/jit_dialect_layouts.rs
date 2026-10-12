//! Each dialect's `for` loops keep their control values where its PUC
//! compiler puts them, and 5.1–5.3 operators take a constant of any type on
//! either side: traces and the method JIT compile these forms and give
//! what the interpreter gives.

use luna_jit::runtime::Value;
use luna_jit::runtime::function::JitProtoState;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

// loops summing into `s`, each with whether a trace compiles for it
const LOOPS: &[(&str, bool)] = &[
    ("for i = 1, 500 do s = s + i end", true),
    ("for i = 500, 1, -3 do s = s + i end", true),
    ("for x = 0.5, 300.5 do s = s + x end", true),
    ("for _, v in ipairs(t) do s = s + v end", true),
    ("for k, v in pairs(t) do s = s + k + v end", true),
    ("for k in next, t do s = s + k end", true),
    (
        "for i, v in ipairs(t) do for j = 1, 3 do s = s + v * j + i end end",
        true,
    ),
    (
        "for k, v, w in pairs(t) do if w == nil then s = s + v end end",
        true,
    ),
];

// loop bodies over the counter `i` with a constant operand
const OPERANDS: &[(&str, bool)] = &[
    ("s = s + (1 - i)", true),
    ("s = s + (1000 - i * 2)", true),
    ("s = s + 2 ^ (i % 4)", true),
    ("s = s + 5 % (i % 4 + 1)", true),
    ("if i < 1e300 then s = s + 1 end", true),
    ("if 250 < i then s = s + 1 end", true),
    ("if 3 >= i then s = s + 1 end", true),
    ("if nil == i then s = s + 1 end", true),
    ("s = s + (1 / 0 > i and 1 or 0)", true),
    ("s = s + 1 - (0 / 0 == 0 / 0 and 1 or 0)", true),
    ("s = s + ('2' + i)", false),
    ("if i ~= false then s = s + 1 end", true),
];

fn loop_chunk(body: &str) -> String {
    format!("local t, s = {{}}, 0 for i = 1, 300 do t[i] = i * 2 end {body} return s")
}

fn operand_chunk(body: &str) -> String {
    format!("local s = 0 for i = 1, 500 do {body} end return s")
}

fn run(vm: &mut Vm, src: &str) -> f64 {
    match vm.eval(src) {
        Ok(r) => match r[0] {
            Value::Int(n) => n as f64,
            Value::Float(f) => f,
            v => panic!("{src}: {v:?}"),
        },
        Err(e) => panic!("{src}: {}", vm.error_text(&e)),
    }
}

/// `src` under traces alone: its result, and the traces compiled and
/// refused.
fn traced(v: LuaVersion, src: &str) -> (f64, u64, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    let r = run(&mut vm, src);
    (
        r,
        vm.trace_compiled_count(),
        vm.trace_compile_failed_count(),
    )
}

fn check_traced(bodies: &[(&str, bool)], chunk: fn(&str) -> String) {
    for v in ALL {
        for &(body, compiles) in bodies {
            let src = chunk(body);
            let want = run(&mut Vm::new(v), &src);
            let (got, compiled, failed) = traced(v, &src);
            assert_eq!(
                want.to_bits(),
                got.to_bits(),
                "{v:?} {body}: {want} vs {got}"
            );
            if compiles {
                assert!(
                    compiled >= 1 && failed == 0,
                    "{v:?} {body}: {compiled} traces compiled, {failed} refused"
                );
            }
        }
    }
}

#[test]
fn traces_run_each_dialects_loops() {
    check_traced(LOOPS, loop_chunk);
}

#[test]
fn traces_take_constant_operands_on_either_side() {
    check_traced(OPERANDS, operand_chunk);
}

#[test]
fn the_method_jit_runs_each_dialects_numeric_loops() {
    let bodies = [
        "local s = 0 for i = 1, x do s = s + i end return s",
        "local s = 0 for i = x, 1, -2 do s = s + i end return s",
        "local s = 0.0 for j = 0.5, x do s = s + j end return s",
        "return 1 - x",
        "if 100000 < x then return 1 end return 2",
    ];
    for v in ALL {
        for body in bodies {
            let src = format!(
                "local function f(x) {body} end \
                 local s = 0 for i = 3, 200 do s = s + f(i) end return s, f"
            );
            let want = run(&mut Vm::new(v), &src);
            let mut vm = luna_jit::new_with_jit(v);
            let got = run(&mut vm, &src);
            assert_eq!(
                want.to_bits(),
                got.to_bits(),
                "{v:?} {body}: {want} vs {got}"
            );
            let r = vm.eval(&src).expect("run");
            let Value::Closure(f) = r[1] else {
                panic!("{v:?} {body}: no function")
            };
            // 5.1 / 5.2 numbers are floats: the method JIT leaves
            // integer-looking loops to the interpreter there
            if v >= LuaVersion::Lua53 {
                assert!(
                    matches!(f.proto.jit.get(), JitProtoState::Compiled { .. }),
                    "{v:?} {body}: not compiled"
                );
            }
        }
    }
}

#[test]
fn a_global_past_constant_255_is_read_and_written_in_every_tier() {
    // 5.1 reaches it with `GETGLOBAL` / `SETGLOBAL` of an 18-bit index
    let mut src = String::from("local t = {");
    for i in 0..300 {
        src.push_str(&format!("{i}.5, "));
    }
    src.push_str("}\nG = 0\nfor i = 1, 500 do G = G + i H = G end\nreturn G + H + #t");
    for v in ALL {
        let want = run(&mut Vm::new(v), &src);
        let (got, compiled, failed) = traced(v, &src);
        assert_eq!(want.to_bits(), got.to_bits(), "{v:?}: {want} vs {got}");
        assert!(
            compiled >= 1 && failed == 0,
            "{v:?}: {compiled} compiled, {failed} refused"
        );
        let got = run(&mut luna_jit::new_with_jit(v), &src);
        assert_eq!(want.to_bits(), got.to_bits(), "{v:?} JIT: {want} vs {got}");
    }
}
