//! The method JIT on the constant- and immediate-operand opcodes: a
//! function using them compiles, and gives what the interpreter gives.

use luna_jit::runtime::Value;
use luna_jit::runtime::function::JitProtoState;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

// each chunk defines `f`, calls it over a range of integers and sums
fn chunk(body: &str) -> String {
    format!(
        "local function f(x) {body} end \
         local s = 0 for i = -70, 70 do s = s + f(i) end return s"
    )
}

// bodies over one integer parameter, integer results
const INT_BODIES: &[&str] = &[
    "return x + 1",
    "return 1 + x",
    "return x + 127",
    "return x + 128",
    "return x + -127",
    "return x + 100000",
    "return x - 1",
    "return x - 128",
    "return x - 100000",
    "return x * 3",
    "return 3 * x",
    "return x * -7",
    "return x % 7",
    "return x % -7",
    "return x // 3",
    "return x // -3",
    "return x % 1000",
    "return x & 12",
    "return 12 & x",
    "return x | 1",
    "return x ~ 255",
    "return x << 3",
    "return x >> 1",
    "return x << -2",
    "return x >> 70",
    "if x < 5 then return 1 end return 2",
    "if x <= 5 then return 1 end return 2",
    "if x > 5 then return 1 end return 2",
    "if x >= 5 then return 1 end return 2",
    "if 5 < x then return 1 end return 2",
    "if 5 >= x then return 1 end return 2",
    "if x == 3 then return 1 end return 2",
    "if x ~= 3 then return 1 end return 2",
    "if x == 100000 then return 1 end return x - 100000",
    "if x < -127 then return 1 end if x > 128 then return 3 end return 2",
    "local a = x * 2 + 1 if a % 3 == 0 then return a - 1 end return a + 1",
];

fn run(vm: &mut Vm, src: &str) -> Value {
    vm.eval(src).unwrap_or_else(|e| panic!("{src}: {e:?}"))[0]
}

fn same(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    }
}

#[test]
fn integer_functions_match_the_interpreter() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for body in INT_BODIES {
            let src = chunk(body);
            let want = run(&mut Vm::new(v), &src);
            let got = run(&mut luna_jit::new_with_jit(v), &src);
            assert!(same(want, got), "{v:?} {body}: {want:?} vs {got:?}");
        }
    }
}

#[test]
fn float_functions_match_the_interpreter() {
    let bodies = [
        "return x * 0.5",
        "return 0.5 * x",
        "return x + 0.25",
        "return x - 0.25",
        "return x / 4",
        "return x * 2.0 + 1.5",
        "if x < 2.5 then return x + 0.5 end return x - 0.5",
    ];
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        for body in bodies {
            let src = format!(
                "local function f(x) {body} end \
                 local s = 0.5 for i = 1, 200 do s = s + f(i + 0.5) end return s"
            );
            let want = run(&mut Vm::new(v), &src);
            let got = run(&mut luna_jit::new_with_jit(v), &src);
            assert!(same(want, got), "{v:?} {body}: {want:?} vs {got:?}");
        }
    }
}

// the shapes the method JIT took before these opcodes existed still compile
#[test]
fn functions_on_constant_operands_are_compiled() {
    for body in [
        "return x + 1",
        "return x * 3 - 1",
        "if x < 2 then return x end return x - 2",
        "if x == 3 then return 1 end return x * 2",
    ] {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
        let outer = vm.load(chunk(body).as_bytes(), b"=t").expect("load");
        vm.call_value(Value::Closure(outer), &[]).expect("run");
        let f = outer.proto.protos[0];
        assert!(
            matches!(f.jit.get(), JitProtoState::Compiled { .. }),
            "{body}"
        );
    }
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let src = "local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end \
               return fib(20), fib";
    let r = vm.eval(src).expect("eval");
    assert!(matches!(r[0], Value::Int(6765)), "{r:?}");
    let Value::Closure(fib) = r[1] else {
        panic!("{r:?}")
    };
    assert!(matches!(
        fib.proto.jit.get(),
        JitProtoState::Compiled { .. }
    ));
}
