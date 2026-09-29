//! An exit that resumes at the trace's own head has not run the head
//! op, so the dispatcher must let the interpreter run it before entering
//! the trace again. The `ipairs` value-type check in `TForCall` returned
//! to the head without saying so: a loop whose body is only that call
//! (`for _, b in ipairs(arr) do o = o end`) met a value of another type
//! and handed the same pc back and forth forever.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, hot: Option<u32>) -> String {
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
    // a livelock shows up as a budget error instead of a hung test
    vm.set_instr_budget(Some(1_000_000));
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    }
}

#[test]
fn ipairs_value_of_another_type_leaves_the_trace_once() {
    let src = r#"
        local arr = {1, 2.5, "3", 4}
        local o, n = 1, 0
        for _, a in ipairs(arr) do
          for _, b in ipairs(arr) do o = o end
          n = n + 1
        end
        return tostring(n)"#;
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(run(v, src, None), "4", "{v:?}: interpreter");
        for hot in [1, 2, 16] {
            assert_eq!(run(v, src, Some(hot)), "4", "{v:?}, hot {hot}");
        }
    }
}
