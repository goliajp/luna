//! Register values the compiler never produces, written by `debug.setlocal`
//! or by a crafted binary chunk the verifier accepts (it checks operands,
//! not the values registers hold at run time). PUC reads such state
//! unchecked and prints garbage, loops or crashes; luna raises a Lua error.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn eval_str(vm: &mut Vm, src: &str) -> String {
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => panic!("uncaught: {}", vm.error_text(&e)),
    }
}

/// The error a protected call of `body` raises, in every dialect.
#[track_caller]
fn raises(body: &str, want: &str) {
    for v in DIALECTS {
        let mut vm = Vm::new(v);
        let src = format!(
            "local ok, e = pcall(function() {body} end) return tostring(ok) .. ' ' .. tostring(e)"
        );
        let got = eval_str(&mut vm, &src);
        assert!(
            got.starts_with("false ") && got.ends_with(want),
            "{v:?}: got {got:?}, want an error ending in {want:?}"
        );
    }
}

#[test]
fn for_index_set_to_a_string() {
    raises(
        "for i = 1, 3 do debug.setlocal(1, 1, 'x') end",
        "'for' state corrupted",
    );
}

#[test]
fn for_limit_set_to_a_table() {
    raises(
        "for i = 1, 3 do debug.setlocal(1, 2, {}) end",
        "'for' state corrupted",
    );
}

#[test]
fn float_for_limit_set_to_nil() {
    raises(
        "for i = 1.5, 3 do debug.setlocal(1, 2, nil) end",
        "'for' state corrupted",
    );
}

/// 5.1 and 5.2 have one number type: a float stored into an integer
/// loop's slot is a number like any other, and PUC keeps looping with it.
/// Expected output from PUC 5.1.5 and 5.2.4.
#[test]
fn mixed_number_state_runs_on_5_1_and_5_2() {
    let index = "local r = '' for i = 1, 3 do if i == 1 then debug.setlocal(1, 2, 1.5) end r = r .. i .. ' ' end return r";
    let limit = "local r = '' for i = 1, 3 do if i == 1 then debug.setlocal(1, 3, 2.5) end r = r .. i .. ' ' end return r";
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        let mut vm = Vm::new(v);
        assert_eq!(eval_str(&mut vm, index), "1 2.5 ", "{v:?}");
        assert_eq!(eval_str(&mut vm, limit), "1 2 ", "{v:?}");
    }
}

/// From 5.3 on an integer loop keeps integer slots; PUC 5.3.6–5.5.1 read
/// a float there as integer bits (garbage or an endless loop).
#[test]
fn mixed_number_state_raises_from_5_3() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let got = eval_str(
            &mut vm,
            "local ok, e = pcall(function() for i = 1, 3 do debug.setlocal(1, 1, 1.5) end end) return e",
        );
        assert!(got.ends_with("'for' state corrupted"), "{v:?}: {got}");
    }
}

/// The table a constructor fills, replaced while an element is evaluated.
/// PUC 5.4.9 segfaults here.
#[test]
fn setlist_target_replaced() {
    raises(
        "local function f()
           for i = 1, 20 do
             local n, v = debug.getlocal(2, i)
             if not n then break end
             if type(v) == 'table' then debug.setlocal(2, i, 1) end
           end
           return 1, 2
         end
         local t = {f()}",
        "attempt to index a number value",
    );
}

/// `string.dump` of `f` (whose chunk name `eval` travels in the dump),
/// with the only instruction equal to `from` replaced by `to`.
fn patched(vm: &mut Vm, f: &str, from: Inst, to: Inst) -> Vec<u8> {
    let v = vm.eval(&format!("return string.dump({f})")).expect("dump");
    let Value::Str(s) = v[0] else {
        panic!("string.dump returned {:?}", v[0]);
    };
    let mut bytes = s.as_bytes().to_vec();
    let (old, new) = (from.0.to_le_bytes(), to.0.to_le_bytes());
    let at: Vec<usize> = (0..bytes.len() - 3)
        .filter(|&i| bytes[i..i + 4] == old)
        .collect();
    assert_eq!(at.len(), 1, "instruction {from:?} not unique in the dump");
    bytes[at[0]..at[0] + 4].copy_from_slice(&new);
    bytes
}

fn run_chunk(vm: &mut Vm, bytes: &[u8]) -> String {
    let f = vm
        .load(bytes, b"=crafted")
        .expect("the verifier accepts it");
    match vm.call_value(Value::Closure(f), &[]) {
        Ok(_) => panic!("crafted chunk ran without error"),
        Err(e) => vm.error_text(&e),
    }
}

/// The loop body stores a string into the hidden index slot. R[0..=2] is
/// the loop state, R[3] `i`, R[4] `x`.
#[test]
fn crafted_chunk_writes_the_for_index() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let bytes = patched(
        &mut vm,
        "function() for i = 1, 3 do local x = 'x' end end",
        Inst::iabx(Op::LoadK, 4, 0),
        Inst::iabx(Op::LoadK, 0, 0),
    );
    let msg = run_chunk(&mut vm, &bytes);
    assert_eq!(msg, "eval:1: 'for' state corrupted");
}

/// `NewTable` replaced by a `LoadI`: the constructor's SetList finds a
/// number.
#[test]
fn crafted_chunk_setlist_on_a_number() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let bytes = patched(
        &mut vm,
        "function() local t = {1, 2, 3} return t end",
        Inst::iabc(Op::NewTable, 0, 3, 0, false),
        Inst::iasbx(Op::LoadI, 0, 7),
    );
    let msg = run_chunk(&mut vm, &bytes);
    assert_eq!(msg, "eval:1: attempt to index a number value");
}

/// A second `TBC` on a slot already waiting to be closed.
#[test]
fn crafted_chunk_registers_a_close_slot_twice() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let bytes = patched(
        &mut vm,
        "function(o) local x <close> = o local y = 1 end",
        Inst::iasbx(Op::LoadI, 2, 1),
        Inst::iabc(Op::Tbc, 1, 0, 0, false),
    );
    let f = vm
        .load(&bytes, b"=crafted")
        .expect("the verifier accepts it");
    let o = vm
        .eval("return setmetatable({}, {__close = function() end})")
        .expect("closable")[0];
    let msg = match vm.call_value(Value::Closure(f), &[o]) {
        Ok(_) => panic!("crafted chunk ran without error"),
        Err(e) => vm.error_text(&e),
    };
    assert_eq!(msg, "eval:1: '<close>' state corrupted");
}
