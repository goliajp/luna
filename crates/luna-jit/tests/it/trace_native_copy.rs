//! A native function copied between registers by a trace: a hot loop
//! whose body is long enough to be dispatched (a trace that stops at a
//! call runs only past a length gate) copies a native into the register
//! it calls it from, or passes it on, and the trace leaves before the
//! call. The copy must reach the interpreter as the native, not as what
//! the register held before.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;
use luna_jit::vm::{LuaError, Vm};

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// Straight-line additions before the call: the gate lies between 34
/// and 36 of them.
const PADS: [usize; 6] = [34, 35, 36, 45, 80, 160];

fn one(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(1)]))
}

fn three(vm: &mut Vm, fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(1), Value::Int(2), Value::Int(3)]))
}

fn count(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(i64::from(nargs))]))
}

/// The loop bodies, each ending in `c[1] = c[1] + <expr>`; `f` is a
/// native returning 1, `three` one returning 1, 2, 3, `count` one
/// returning its argument count, `o` a table whose `__call` is `f` and
/// `m` one whose `__index` is `f`.
const SHAPES: [(&str, &str); 9] = [
    ("call", "c[1] = c[1] + f(1)"),
    ("copy", "local g = f c[1] = c[1] + g(1)"),
    ("copy-test", "local g = f if g then c[1] = c[1] + g(1) end"),
    ("copy-eq", "local g = f if g == f then c[1] = c[1] + 1 end"),
    ("index-mm", "c[1] = c[1] + m.k"),
    ("call-mm", "c[1] = c[1] + o(1)"),
    ("multret", "local p, q, r = three() c[1] = c[1] + p + q + r"),
    ("vararg", "c[1] = c[1] + count(1, 2, 3, x, x)"),
    ("inlined", "c[1] = c[1] + h(f)"),
];

fn source(pad: usize, body: &str) -> String {
    format!(
        "local c, f, three, count, o, m = ...
         local function h(g) return g(1) end
         for i = 1, 2000 do
           local x = i {}
           {body}
         end
         return c[1]",
        "x = x + 1 ".repeat(pad)
    )
}

fn run(v: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(jit);
    // the values are held across calls that may collect: pinned
    let keep = |vm: &mut Vm, v: Value| {
        vm.pin_host(v);
        v
    };
    let f = vm.native(one);
    let f = keep(&mut vm, f);
    let three = vm.native(three);
    let three = keep(&mut vm, three);
    let count = vm.native(count);
    let count = keep(&mut vm, count);
    let mt = vm.eval("return function(f) return setmetatable({}, {__call = f}), setmetatable({}, {__index = f}) end")
        .expect("constructor")[0];
    let om = vm.call_value(mt, &[f]).expect("o, m");
    let om = [keep(&mut vm, om[0]), keep(&mut vm, om[1])];
    let c = vm.eval("return {0}").expect("c")[0];
    let c = keep(&mut vm, c);
    let main = vm.load(src.as_bytes(), b"=loop").expect("load");
    let out = match vm.call_value(Value::Closure(main), &[c, f, three, count, om[0], om[1]]) {
        Ok(r) => format!("{:?}", r.first()),
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

#[test]
fn a_native_copied_by_a_trace_reaches_the_interpreter() {
    let mut failures = Vec::new();
    for v in DIALECTS {
        for (name, body) in SHAPES {
            for pad in PADS {
                let src = source(pad, body);
                let (interp, _) = run(v, &src, false);
                let (jit, dispatched) = run(v, &src, true);
                if jit != interp {
                    failures.push(format!(
                        "{v:?} {name} pad {pad}: {jit} (interpreter {interp})"
                    ));
                }
                if pad >= 45 && dispatched == 0 {
                    failures.push(format!("{v:?} {name} pad {pad}: no trace was dispatched"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
