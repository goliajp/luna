//! A numeric `for` with a float start or step steps with `ForLoopF` /
//! `ForLoop55F` from 5.3 on, which test for a float loop first. The loop
//! runs as with `ForLoop` / `ForLoop55`, also when `debug.setlocal` stores
//! a number of the loop's kind into its control registers mid-loop (the
//! step then takes the slow path once). Expected output from PUC 5.1.5,
//! 5.2.4, 5.3.6, 5.4.9 and 5.5.1.

use luna_core::compiler::compile_chunk_with_last_target;
use luna_core::frontend::parser::parse;
use luna_core::runtime::{Heap, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::Op;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// The numeric `for` step opcode the main chunk of `src` compiles to.
fn loop_op(src: &str, v: LuaVersion) -> Op {
    let ast = parse(src.as_bytes(), v).expect("parse");
    let mut heap = Heap::new();
    let (proto, _) =
        compile_chunk_with_last_target(&ast, v, b"=for_kinds", &mut heap).expect("compile");
    let ops: Vec<Op> = proto
        .code
        .iter()
        .map(|i| i.op())
        .filter(|o| o.is_for_loop())
        .collect();
    assert_eq!(ops.len(), 1, "{v:?}: {src}");
    ops[0]
}

#[test]
fn a_float_start_or_step_picks_the_float_first_step() {
    for v in DIALECTS {
        let (plain, float) = match v {
            LuaVersion::Lua51 | LuaVersion::Lua52 => (Op::ForLoop, Op::ForLoop),
            LuaVersion::Lua55 => (Op::ForLoop55, Op::ForLoop55F),
            _ => (Op::ForLoop, Op::ForLoopF),
        };
        assert_eq!(loop_op("for i = 1, 10 do end", v), plain, "{v:?}");
        assert_eq!(
            loop_op("local n = 3 for i = n, 10 do end", v),
            plain,
            "{v:?}"
        );
        assert_eq!(loop_op("for i = 1, 10.5 do end", v), plain, "{v:?}");
        assert_eq!(loop_op("for x = 0.5, 10 do end", v), float, "{v:?}");
        assert_eq!(loop_op("for x = 1, 10, 0.5 do end", v), float, "{v:?}");
    }
}

fn run(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{v:?}: expected a string, got {other:?}"),
        },
        Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
    }
}

#[test]
fn values_stored_into_the_control_registers_run_on() {
    let int = "local n = 0 for i = 1, 10 do n = n + 1 if n == 2 then debug.setlocal(1, 2, 3) end end return tostring(n)";
    let flt = "local n = 0 for x = 0.5, 10.5 do n = n + 1 if n == 2 then debug.setlocal(1, 2, 3.5) end end return tostring(n)";
    for (v, want_int, want_flt) in [
        (LuaVersion::Lua51, "9", "9"),
        (LuaVersion::Lua52, "9", "9"),
        (LuaVersion::Lua53, "9", "9"),
        (LuaVersion::Lua54, "10", "9"),
        (LuaVersion::Lua55, "5", "4"),
    ] {
        assert_eq!(run(v, int), want_int, "{v:?}");
        assert_eq!(run(v, flt), want_flt, "{v:?}");
    }
}

#[test]
fn a_float_loop_counts_down() {
    let src =
        r#"local t = {} for x = 4.5, 0, -1.5 do t[#t + 1] = x end return table.concat(t, " ")"#;
    for v in DIALECTS {
        let want = if v <= LuaVersion::Lua52 {
            "4.5 3 1.5 0"
        } else {
            "4.5 3.0 1.5 0.0"
        };
        assert_eq!(run(v, src), want, "{v:?}");
    }
}

/// The local `debug.setlocal` reaches a loop's limit by, inside `function()
/// for i = ... do ... end end`: 5.1–5.3 `(for limit)`, 5.4 the second
/// `(for state)` (an integer loop's count), 5.5 the first.
fn limit_local(v: LuaVersion) -> u32 {
    if v == LuaVersion::Lua55 { 1 } else { 2 }
}

/// A string stored into a loop's limit or step, by `debug.setlocal`, raises
/// at the loop's next step, integer and float loops alike: the steps that do
/// not test a register's tag see the change through the register after it
/// (see `num_words::unseal`).
#[test]
fn a_limit_set_by_debug_setlocal_raises() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        // the limit, and the step after it
        for l in [limit_local(v), limit_local(v) + 1] {
            for range in ["1, 3", "0.5, 3", "3.5, 0, -1"] {
                let src = format!(
                    "local ok, e = pcall(function() for i = {range} do debug.setlocal(1, {l}, 'x') end end) \
                 return tostring(ok) .. ' ' .. tostring(e)"
                );
                let got = run(v, &src);
                assert!(
                    got.starts_with("false ") && got.ends_with("'for' state corrupted"),
                    "{v:?} {range} local {l}: {got}"
                );
            }
        }
    }
}

/// A float loop's limit overwritten with the integer 7 by the loop body's
/// own code, in a crafted chunk: the step reads the limit without testing
/// its tag, as PUC does, and the loop ends after one round as on PUC 5.3.6,
/// 5.4.9 and 5.5.1 (there with `debug.setlocal`, which PUC does not check).
#[test]
fn a_float_loop_limit_overwritten_by_the_body_runs_as_on_puc() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let r = vm
            .eval("return function() local n = 0 for x = 0.5, 10.5 do n = n + 1 local y = 7 end return n end")
            .expect("compile");
        let Value::Closure(g) = r[0] else {
            panic!("{v:?}: expected a function");
        };
        // `n` is R0, the loop's registers start at R1, `y` comes after them
        let limit = if v == LuaVersion::Lua55 { 1 } else { 2 };
        let y = if v == LuaVersion::Lua55 { 4 } else { 5 };
        let at: Vec<_> = g
            .proto
            .code
            .iter()
            .filter(|i| i.a() == y)
            .map(|i| i.0)
            .collect();
        assert_eq!(at.len(), 1, "{v:?}: the store of `y`");
        let mut bytes = luna_core::vm::dump::dump(&g.proto, false, v);
        let (old, new) = (
            at[0].to_le_bytes(),
            ((at[0] & !(0xff << 7)) | (limit << 7)).to_le_bytes(),
        );
        let pos: Vec<usize> = (0..bytes.len() - 3)
            .filter(|&i| bytes[i..i + 4] == old)
            .collect();
        assert_eq!(pos.len(), 1, "{v:?}: the store of `y` in the dump");
        bytes[pos[0]..pos[0] + 4].copy_from_slice(&new);
        let f = vm
            .load(&bytes, b"=crafted")
            .expect("the verifier accepts it");
        match vm.call_value(Value::Closure(f), &[]) {
            Ok(r) => assert!(matches!(r.first(), Some(Value::Int(1))), "{v:?}: {r:?}"),
            Err(e) => panic!("{v:?}: {}", vm.error_text(&e)),
        }
    }
}
