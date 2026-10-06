//! An exhausted instruction budget or memory cap stays exhausted until the
//! host arms a new one: every instruction run before that raises the limit's
//! error again, whoever runs it (a `pcall` caller, an `xpcall` handler, a
//! `__close` or `__gc` handler, a metamethod, a library callback, a
//! coroutine), so a script cannot catch the error and carry on.
//!
//! Every case runs on its own thread with a timeout: before the fix the
//! scripts that catch the error run forever.

use std::sync::mpsc;
use std::time::Duration;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::error::LuaErrorKind;

const BUDGET: i64 = 100_000;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq)]
enum Outcome {
    Ok(String),
    Err { text: String, kind: LuaErrorKind },
    Timeout,
}

fn sandbox(version: LuaVersion) -> Vm {
    Vm::sandbox(version)
        .open_base()
        .open_math()
        .open_string()
        .open_table()
        .open_coroutine()
        .build()
}

fn show(vals: &[Value]) -> String {
    vals.iter()
        .map(|v| match v {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => format!("{f}"),
            Value::Bool(b) => b.to_string(),
            Value::Nil => "nil".to_string(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn call(vm: &mut Vm, src: &str) -> Outcome {
    let cl = vm.load(src.as_bytes(), b"=case").expect("compile");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(vals) => Outcome::Ok(show(&vals)),
        Err(e) => Outcome::Err {
            text: vm.error_text(&e),
            kind: vm.error_kind(),
        },
    }
}

/// Run `body` against a fresh sandbox on its own thread; `Timeout` when it
/// does not come back. The Vm is dropped inside `body`'s thread before the
/// outcome is sent, so a finalizer that never stops at close counts too. A
/// script that never stops keeps its thread spinning until the test process
/// exits.
fn bounded<F>(version: LuaVersion, body: F) -> Outcome
where
    F: FnOnce(&mut Vm) -> Outcome + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut vm = sandbox(version);
        let out = body(&mut vm);
        drop(vm);
        let _ = tx.send(out);
    });
    rx.recv_timeout(TIMEOUT).unwrap_or(Outcome::Timeout)
}

/// Arm the budget, run `src`, then drop the Vm (which runs the finalizers).
fn budgeted(version: LuaVersion, src: &'static str) -> Outcome {
    bounded(version, move |vm| {
        vm.set_instr_budget(Some(BUDGET));
        call(vm, src)
    })
}

fn exhausted(out: &Outcome) {
    match out {
        Outcome::Err { text, kind } => {
            assert!(text.contains("instruction budget exceeded"), "{text}");
            assert_eq!(*kind, LuaErrorKind::InstrBudget, "{text}");
        }
        other => panic!("expected the budget error, got {other:?}"),
    }
}

#[test]
fn pcall_cannot_resume_after_exhaustion() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "pcall(function() while true do end end) while true do end",
    ));
}

#[test]
fn repeated_pcall_does_not_return_normally() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "for i = 1, 3 do pcall(function() while true do end end) end return 1",
    ));
}

#[test]
fn xpcall_handler_is_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "xpcall(function() while true do end end, function(e) while true do end end)",
    ));
}

#[test]
fn close_handler_is_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua54,
        "do local x <close> = setmetatable({}, {__close = function() while true do end end})
           while true do end end",
    ));
}

#[test]
fn gc_finalizer_is_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua54,
        "setmetatable({}, {__gc = function() while true do end end}) collectgarbage()
         while true do end",
    ));
}

#[test]
fn pcall_result_is_never_delivered() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "local ok, e = pcall(function() while true do end end) return ok, e",
    ));
}

#[test]
fn a_new_budget_clears_the_exhausted_state() {
    let out = bounded(LuaVersion::Lua51, |vm| {
        vm.set_instr_budget(Some(BUDGET));
        exhausted(&call(vm, "while true do end"));
        assert_eq!(vm.instr_budget_remaining(), Some(0));
        vm.set_instr_budget(Some(BUDGET));
        call(vm, "local s = 0 for i = 1, 30000 do s = s + i end return s")
    });
    assert_eq!(out, Outcome::Ok("450015000".to_string()));
}

#[test]
fn coroutine_body_and_resumer_are_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "local co = coroutine.create(function() while true do end end)
         local ok, e = coroutine.resume(co)
         assert(not ok and e == 'instruction budget exceeded', e)
         while true do end",
    ));
    exhausted(&budgeted(
        LuaVersion::Lua54,
        "pcall(coroutine.wrap(function() while true do end end))
         local co = coroutine.wrap(function() local n = 0 while true do n = n + 1 end end)
         while true do co() end",
    ));
}

#[test]
fn metamethods_are_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "local t = setmetatable({}, {
           __index = function() while true do end end,
           __add = function() while true do end end,
         })
         pcall(function() return t.x end)
         pcall(function() return t + 1 end)
         while true do end",
    ));
}

#[test]
fn library_callbacks_are_stopped() {
    exhausted(&budgeted(
        LuaVersion::Lua51,
        "local t = {3, 1, 2}
         pcall(table.sort, t, function(a, b) while true do end end)
         pcall(string.gsub, 'abc', '%w', function() while true do end end)
         local n = 0
         while true do n = n + 1 end",
    ));
    // the error raised inside the callback keeps its kind through the
    // library function that called it
    exhausted(&budgeted(
        LuaVersion::Lua54,
        "table.sort({3, 1, 2}, function(a, b) while true do end end)",
    ));
}

#[test]
fn finalizers_at_close_are_stopped() {
    let out = bounded(LuaVersion::Lua54, |vm| {
        vm.set_instr_budget(Some(BUDGET));
        // the table stays reachable until the Vm is dropped, so its
        // finalizer runs at close, after the budget ran out
        let out = call(
            vm,
            "keep = setmetatable({}, {__gc = function() while true do end end}) while true do end",
        );
        exhausted(&out);
        out
    });
    exhausted(&out);
}

#[test]
fn memory_cap_stays_exceeded() {
    let out = bounded(LuaVersion::Lua54, |vm| {
        vm.set_memory_cap(Some(vm.memory_used() + 256 * 1024));
        let out = call(
            vm,
            "pcall(function() local t = {} for i = 1, 1e9 do t[i] = {} end end)
             while true do end",
        );
        match &out {
            Outcome::Err { text, kind } => {
                assert!(text.contains("memory cap exceeded"), "{text}");
                assert_eq!(*kind, LuaErrorKind::MemoryCap, "{text}");
            }
            other => panic!("expected the memory cap error, got {other:?}"),
        }
        vm.set_memory_cap(Some(vm.memory_used() + 256 * 1024));
        call(vm, "local s = 0 for i = 1, 30000 do s = s + i end return s")
    });
    assert_eq!(out, Outcome::Ok("450015000".to_string()));
}
