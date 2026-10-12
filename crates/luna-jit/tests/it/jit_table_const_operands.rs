//! Table reads and writes with a constant operand (a stored constant, a
//! constant key before 5.4) or an upvalue table with a key that is not a
//! string constant (5.2 / 5.3): traces and the method JIT compile them and
//! give what the interpreter gives.

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

// loop bodies over a table `t` and a counter `i`, adding to `s`, each
// with whether its trace compiles (the two that do not, a string's length
// and a boolean key, did not with their constants in registers either)
const STORES: &[(&str, bool)] = &[
    ("t.a = 1.5 s = s + t.a", true),
    ("t.b = 'x' s = s + #t.b", false),
    ("t.c = true if t.c then s = s + 1 end", true),
    ("t.d = 7 s = s + t.d", true),
    ("t[1] = 2 s = s + t[1]", true),
    ("t[i] = 3 s = s + t[i]", true),
    ("t[i] = 0.25 s = s + t[i]", true),
    ("t[i] = false if not t[i] then s = s + 1 end", true),
];

const KEYS: &[(&str, bool)] = &[
    ("t[2.5] = i s = s + t[2.5]", true),
    ("t[-3] = 1.5 s = s + t[-3]", true),
    ("t[1000] = i s = s + t[1000]", true),
    ("t[true] = 2 s = s + t[true]", false),
];

// an upvalue table indexed by a register
const REGISTER_KEYS: &[(&str, bool)] = &[
    ("s = s + t[i % 2 + 1]", true),
    ("t[i] = i s = s + t[i]", true),
];

fn local_chunk(body: &str) -> String {
    format!("local t, s = {{0}}, 0 for i = 1, 400 do {body} end return s")
}

// the same bodies with `t` an upvalue of the looping function
fn upvalue_chunk(body: &str) -> String {
    format!(
        "local t = {{0, 0}} \
         local function f(n) local s = 0 for i = 1, n do {body} end return s end \
         return f(400)"
    )
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
fn traces_store_constants() {
    check_traced(STORES, local_chunk);
}

#[test]
fn traces_take_constant_keys() {
    check_traced(KEYS, local_chunk);
}

#[test]
fn traces_index_upvalue_tables() {
    check_traced(STORES, upvalue_chunk);
    check_traced(KEYS, upvalue_chunk);
    check_traced(REGISTER_KEYS, upvalue_chunk);
}

#[test]
fn the_method_jit_compiles_constant_stores() {
    // whether the function compiles; the second did not compile when its
    // constants were loaded into registers either
    for (body, compiles) in [
        ("local t = {} t[x] = 0.5 return t[x]", true),
        ("local t = {} t[x] = 2 t[1] = 0 return t[x] + t[1]", false),
    ] {
        for v in ALL {
            let src = format!(
                "local function f(x) {body} end \
                 local s = 0 for i = 3, 200 do s = s + f(i) end return s"
            );
            let want = run(&mut Vm::new(v), &src);
            let mut vm = luna_jit::new_with_jit(v);
            let outer = vm.load(src.as_bytes(), b"=t").expect("load");
            let got = match vm.call_value(Value::Closure(outer), &[]).expect("run")[0] {
                Value::Int(n) => n as f64,
                Value::Float(f) => f,
                v => panic!("{v:?}"),
            };
            assert_eq!(
                want.to_bits(),
                got.to_bits(),
                "{v:?} {body}: {want} vs {got}"
            );
            let f = outer.proto.protos[0];
            if compiles {
                assert!(
                    matches!(f.jit.get(), JitProtoState::Compiled { .. }),
                    "{v:?} {body}"
                );
            }
        }
    }
}
