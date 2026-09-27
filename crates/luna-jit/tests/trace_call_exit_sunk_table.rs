//! A trace that ends at a call leaves the interpreter to run the call
//! with every register of the frame live. A table the trace had not
//! allocated yet (escape analysis sank it) must exist by then: in
//! `s[#s + 1] = {f()}` the constructor's `SetList` runs after the call,
//! and it was handed a register that never held the table.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    vm.jit.trace_hot_threshold = 1;
    vm.jit.call_hot_threshold = 1;
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

#[track_caller]
fn same(versions: &[LuaVersion], src: &str, want: &str) {
    for &v in versions {
        let (interp, _) = run(v, src, false);
        let (jit, dispatched) = run(v, src, true);
        assert_eq!(interp, want, "{v:?}: interpreter");
        assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
        assert!(dispatched > 0, "{v:?}: no trace was dispatched");
    }
}

const WANT: &str = "40 1 -2 19680 16016";

// enough work before the call that the truncated trace is dispatched
const PAD: &str = "
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
            a = a + i b = b ~ a
";

#[test]
fn constructor_holding_a_call_in_a_numeric_for() {
    let src = format!(
        r##"
        local function f(a) return 1, a end
        local function k()
          local s, a, b = {{}}, 0, 0
          for i = 1, 40 do
            {PAD}
            s[#s + 1] = {{f(-2)}}
          end
          return #s .. " " .. s[40][1] .. " " .. s[40][2] .. " " .. a .. " " .. b
        end
        return k()"##
    );
    same(&[LuaVersion::Lua54, LuaVersion::Lua55], &src, WANT);
}

#[test]
fn constructor_holding_a_call_in_a_while_loop() {
    let src = format!(
        r##"
        local function va(...) return select("#", ...), ... end
        local function k()
          local s, a, b, i = {{}}, 0, 0, 0
          while i < 40 do
            i = i + 1
            {PAD}
            s[#s + 1] = {{va(-2)}}
          end
          return #s .. " " .. s[40][1] .. " " .. s[40][2] .. " " .. a .. " " .. b
        end
        return k()"##
    );
    same(&[LuaVersion::Lua54, LuaVersion::Lua55], &src, WANT);
}
