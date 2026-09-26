//! Register state the compiler never produces, with the JITs on: a loop
//! whose control slots `debug.setlocal` overwrites after a trace for it is
//! running, and a table constructor whose target table is not presized.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;
use luna_jit::runtime::function::JitProtoState;
use luna_jit::vm::isa::{Inst, Op};

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

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

/// The loop runs as a trace for most of its iterations before the call
/// that corrupts it; the interpreter's back edge then sees the state.
#[test]
fn for_state_corrupted_after_the_trace_ran() {
    let src = r#"
        local function f()
          local n = 0
          for i = 1, 3000 do
            n = n + i
            if i == 2000 then debug.setlocal(1, 2, "x") end
          end
          return n
        end
        local ok, e = pcall(f)
        return tostring(ok) .. " " .. tostring(e)
    "#;
    for v in DIALECTS {
        let (interp, _) = run(v, src, false);
        let (jit, dispatched) = run(v, src, true);
        assert_eq!(jit, interp, "{v:?}: JIT differs from the interpreter");
        // the trace JIT dispatches numeric loops from 5.4 on
        if v >= LuaVersion::Lua54 {
            assert!(dispatched > 0, "{v:?}: no trace was dispatched");
        }
        assert!(
            interp.starts_with("false ") && interp.ends_with("'for' state corrupted"),
            "{v:?}: {interp}"
        );
    }
}

/// `function() return {10, 20, 30} end` with its `NewTable` presize
/// dropped: the method JIT's SetList finds an empty array part and must
/// not store past it.
#[test]
fn method_jit_setlist_without_presize() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = luna_jit::new_with_jit(v);
        let r = vm
            .eval("return string.dump(function() return {10, 20, 30} end)")
            .expect("dump");
        let Value::Str(s) = r[0] else {
            panic!("string.dump returned {:?}", r[0]);
        };
        let mut bytes = s.as_bytes().to_vec();
        let old = Inst::iabc(Op::NewTable, 0, 3, 0, false).0.to_le_bytes();
        let new = Inst::iabc(Op::NewTable, 0, 0, 0, false).0.to_le_bytes();
        let at: Vec<usize> = (0..bytes.len() - 3)
            .filter(|&i| bytes[i..i + 4] == old)
            .collect();
        assert_eq!(at.len(), 1, "{v:?}: NewTable not found once in the dump");
        bytes[at[0]..at[0] + 4].copy_from_slice(&new);
        let f = vm
            .load(&bytes, b"=crafted")
            .expect("the verifier accepts it");
        for _ in 0..3 {
            let r = vm.call_value(Value::Closure(f), &[]).expect("call");
            let Some(&Value::Table(t)) = r.first() else {
                panic!("{v:?}: expected a table, got {r:?}");
            };
            assert_eq!(t.len(), 3, "{v:?}");
            for (k, want) in [(1, 10), (2, 20), (3, 30)] {
                assert!(
                    matches!(t.get_int(k), Value::Int(x) if x == want),
                    "{v:?}: t[{k}]"
                );
            }
        }
        assert!(
            matches!(f.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{v:?}: the function did not compile (state {:?})",
            f.proto.jit.get()
        );
    }
}
