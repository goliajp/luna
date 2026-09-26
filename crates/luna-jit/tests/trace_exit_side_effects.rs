//! A trace that leaves in the middle of an iteration must not repeat the
//! side effects of the ops before the exit: each snippet does a
//! non-idempotent store first, then an op whose operand changes shape
//! (gains a metatable, needs a metamethod) after the trace is running.
//! The JIT and the interpreter must agree, and a trace must have run.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

// the dialects whose numeric loops the trace JIT dispatches
const DIALECTS: [LuaVersion; 2] = [LuaVersion::Lua54, LuaVersion::Lua55];

#[track_caller]
fn same(src: &str, want: &str) {
    for v in DIALECTS {
        let (interp, _) = run(v, src, false);
        let (jit, dispatched) = run(v, src, true);
        assert_eq!(interp, want, "{v:?}: interpreter");
        assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
        assert!(dispatched > 0, "{v:?}: no trace was dispatched");
    }
}

const OBJS: &str = r#"
    local objs = {}
    local function fill(plain, meta)
      for i = 1, 300 do objs[i] = i <= 200 and plain or meta end
    end
"#;

#[test]
fn field_read_meets_index_metamethod() {
    let src = format!(
        "{OBJS}
        fill({{x = 1}}, setmetatable({{}}, {{__index = function() return 1 end}}))
        local c, s = {{0}}, 0
        for i = 1, 300 do
          c[1] = c[1] + 1
          s = s + objs[i].x
        end
        return c[1] .. ' ' .. s"
    );
    same(&src, "300 300");
}

#[test]
fn integer_read_meets_index_metamethod() {
    let src = format!(
        "{OBJS}
        fill({{1}}, setmetatable({{}}, {{__index = function() return 1 end}}))
        local c, s = {{0}}, 0
        for i = 1, 300 do
          c[1] = c[1] + 1
          s = s + objs[i][1]
        end
        return c[1] .. ' ' .. s"
    );
    same(&src, "300 300");
}

#[test]
fn store_meets_newindex_metamethod() {
    let src = format!(
        "{OBJS}
        local hits = {{n = 0}}
        fill({{}}, setmetatable({{}}, {{__newindex = function() hits.n = hits.n + 1 end}}))
        local c = {{0}}
        for i = 1, 300 do
          c[1] = c[1] + 1
          objs[i][1] = i
        end
        return c[1] .. ' ' .. hits.n"
    );
    same(&src, "300 100");
}

#[test]
fn length_meets_len_metamethod() {
    let src = format!(
        "{OBJS}
        fill({{1}}, setmetatable({{}}, {{__len = function() return 1 end}}))
        local c, s = {{0}}, 0
        for i = 1, 300 do
          c[1] = c[1] + 1
          s = s + #objs[i]
        end
        return c[1] .. ' ' .. s"
    );
    same(&src, "300 300");
}

/// The concatenation folds from the right: `'a' .. 'b'` is done before
/// the table operand needs `__concat`. (This loop does not compile to a
/// trace today; it pins the result for when it does.)
#[test]
fn concat_meets_concat_metamethod_midway() {
    let src = format!(
        "{OBJS}
        local mt = {{__concat = function(a, b)
          return (type(a) == 'table' and 'T' or a) .. '|' .. (type(b) == 'table' and 'T' or b)
        end}}
        fill('s', setmetatable({{}}, mt))
        local c, n = {{0}}, 0
        for i = 1, 300 do
          c[1] = c[1] + 1
          local r = objs[i] .. 'a' .. 'b'
          if r == 'T|ab' or r == 'sab' then n = n + 1 end
        end
        return c[1] .. ' ' .. n"
    );
    for v in DIALECTS {
        let (interp, _) = run(v, &src, false);
        let (jit, _) = run(v, &src, true);
        assert_eq!(interp, "300 300", "{v:?}: interpreter");
        assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
    }
}

/// `pcall` as a generic-for iterator: the call it makes runs in the
/// interpreter, not inside the trace's iterator helper.
#[test]
fn pcall_as_the_iterator() {
    let src = r#"
        local n = 0
        local function f() n = n + 1 if n > 300 then error("stop", 0) end return n end
        local s = 0
        for ok, v in pcall, f do
          if not ok then s = s .. ":" .. v break end
          s = s + v
        end
        return tostring(s) .. " " .. n
    "#;
    for v in DIALECTS {
        let (interp, _) = run(v, src, false);
        let (jit, _) = run(v, src, true);
        assert_eq!(interp, "45150:stop 301", "{v:?}: interpreter");
        assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
    }
}

/// Table operations on values that are not tables at the time the trace
/// is recorded: a string's length, a field of a string (through the
/// string metatable), an index of a number with a metatable.
#[test]
fn table_ops_on_non_tables() {
    for (src, want) in [
        (
            "local s, n = 'abc', 0 for i = 1, 300 do n = n + #s end return tostring(n)",
            "900",
        ),
        (
            "local s, n = 'abc', 0 for i = 1, 300 do local f = s.len if f then n = n + 1 end end return tostring(n)",
            "300",
        ),
        (
            "debug.setmetatable(0, {__index = function(_, k) return k end})
             local x, n = 5, 0 for i = 1, 300 do n = n + x[2] end
             debug.setmetatable(0, nil) return tostring(n)",
            "600",
        ),
    ] {
        for v in DIALECTS {
            let (interp, _) = run(v, src, false);
            let (jit, _) = run(v, src, true);
            assert_eq!(interp, want, "{v:?}: interpreter");
            assert_eq!(
                jit, interp,
                "{v:?}: JIT differs from the interpreter: {src}"
            );
        }
    }
}
