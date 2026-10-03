//! A function compiled while a parameter held a float, called again with
//! an integer there: the compiled code must not read the integer's bits
//! as a float. `q` is only copied into a local, so the parameter's kind
//! comes from the local's.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(src: &str, method: bool, trace: bool, trace_hot: u32, call_hot: u32) -> String {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(method);
    vm.set_trace_jit_enabled(trace);
    vm.jit.trace_hot_threshold = trace_hot;
    vm.jit.call_hot_threshold = call_hot;
    match vm.eval(src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn float_parameter_called_with_an_integer() {
    let src = r#"
        local function kr(p, q)
          local acc, f1, f2 = 0, p, q
          for i = 1, 12 do
            if not (f2 <= -1.5) then acc = acc + 1 end
            f1 = f1 + 0.5
          end
          return acc
        end
        for r = 1, 12 do kr(-7.75, 0.125) end
        local out = {}
        for _, v in ipairs({math.mininteger, -1, 3, -3}) do out[#out + 1] = kr(v, v) end
        return table.concat(out, " ")"#;
    let interp = run(src, false, false, 1, 16);
    assert_eq!(interp, "0 12 12 0");
    for (method, trace) in [(true, true), (true, false), (false, true)] {
        for (th, ch) in [(1, 16), (1, 1), (7, 16)] {
            let jit = run(src, method, trace, th, ch);
            assert_eq!(
                jit, interp,
                "method {method} trace {trace}, trace hot {th}, call hot {ch}"
            );
        }
    }
}
