//! P3 B6 smoke tests: LuaError Display/Error impls + Vm::error_kind tracking.

use luna_core::vm::{LuaError, LuaErrorKind, Vm};
use luna_core::version::LuaVersion;
use std::error::Error;

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

#[test]
fn display_for_string_error() {
    let mut vm = vm();
    let err = vm.eval("error('hello world')").unwrap_err();
    let s = format!("{err}");
    assert!(s.contains("hello world"), "got: {s}");
}

#[test]
fn display_for_nil_error() {
    let err = LuaError::nil();
    assert_eq!(format!("{err}"), "(nil error)");
}

#[test]
fn error_trait_impl() {
    let mut vm = vm();
    let err = vm.eval("syntax !@#").unwrap_err();
    // std::error::Error trait is implemented; source() returns None.
    let e: &dyn Error = &err;
    assert!(e.source().is_none());
}

#[test]
fn error_kind_syntax() {
    let mut vm = vm();
    let _ = vm.eval("not a valid lua program !@#").unwrap_err();
    assert_eq!(vm.error_kind(), LuaErrorKind::Syntax);
}

#[test]
fn error_kind_instr_budget() {
    let mut vm = vm();
    vm.set_instr_budget(Some(500));
    let _ = vm.eval("while true do end").unwrap_err();
    assert_eq!(vm.error_kind(), LuaErrorKind::InstrBudget);
}

#[test]
fn error_kind_runtime_default() {
    let mut vm = vm();
    let _ = vm.eval("error('boom')").unwrap_err();
    // Runtime is the default; explicit error() call without further
    // classification stays Runtime.
    assert_eq!(vm.error_kind(), LuaErrorKind::Runtime);
}

#[test]
fn error_kind_clears_on_successful_eval() {
    let mut vm = vm();
    let _ = vm.eval("not lua").unwrap_err();
    assert_eq!(vm.error_kind(), LuaErrorKind::Syntax);
    let _ = vm.eval("return 1").unwrap();
    // clear_error_metadata fires on eval entry (B6 contract).
    assert_eq!(vm.error_kind(), LuaErrorKind::Runtime);
}

#[test]
fn error_source_captures_chunkname() {
    let mut vm = vm();
    let _ = vm.eval_chunk("syntax !@#", "myscript.lua").unwrap_err();
    let src = vm.error_source().expect("source should be set");
    assert_eq!(src.0, "myscript.lua");
    // The exact line depends on the lexer; just verify it was captured.
    assert!(src.1 >= 1);
}

#[test]
fn lua_error_kind_display() {
    assert_eq!(format!("{}", LuaErrorKind::Runtime), "runtime");
    assert_eq!(format!("{}", LuaErrorKind::Syntax), "syntax");
    assert_eq!(format!("{}", LuaErrorKind::InstrBudget), "instr-budget");
    assert_eq!(format!("{}", LuaErrorKind::MemoryCap), "memory-cap");
    assert_eq!(format!("{}", LuaErrorKind::Native), "native");
    assert_eq!(format!("{}", LuaErrorKind::OutOfMemory), "out-of-memory");
    assert_eq!(format!("{}", LuaErrorKind::Type), "type");
}

#[test]
fn lua_error_constructors() {
    let nil = LuaError::nil();
    assert!(matches!(nil.0, luna_core::runtime::Value::Nil));
    let n = LuaError::new(luna_core::runtime::Value::Int(7));
    assert!(matches!(n.0, luna_core::runtime::Value::Int(7)));
}
