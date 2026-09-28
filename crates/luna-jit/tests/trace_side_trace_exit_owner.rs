//! A side trace's own exits are not its parent's. A side trace recorded
//! from an exit of a side trace was wired to the parent's exit with the
//! same number, replacing the side trace there: in the function below the
//! `while` loop's exit then returned from the function instead of going
//! on with the next round of the `for` loop.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, hot: Option<u32>) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    match hot {
        Some(h) => {
            vm.jit.trace_hot_threshold = h;
            vm.jit.call_hot_threshold = h;
        }
        None => {
            vm.set_jit_enabled(false);
            vm.set_trace_jit_enabled(false);
        }
    }
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_side_trace_compiled_count())
}

#[test]
fn while_exit_inside_numeric_for_keeps_its_side_trace() {
    let src = r#"
        local seq = {}
        local function f()
          local seq = seq
          for i = 1, 2 do
            local w = 0
            while w < 9 do w = w + 1 seq[#seq + 1] = i end
          end
        end
        for _ = 1, 300 do f() end
        return tostring(#seq)"#;
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let (interp, _) = run(v, src, None);
        assert_eq!(interp, "5400", "{v:?}: interpreter");
        let mut side = 0;
        for hot in [2, 5, 7, 11] {
            let (jit, s) = run(v, src, Some(hot));
            assert_eq!(
                jit, interp,
                "{v:?}, hot {hot}: JIT differs from the interpreter"
            );
            side += s;
        }
        assert!(side > 0, "{v:?}: no side trace was compiled");
    }
}
