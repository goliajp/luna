//! Negative and version-gating tests: the parser must reject what each
//! dialect rejects, with correct line numbers.

use luna::frontend::parse;
use luna::version::LuaVersion::{self, Lua51, Lua54, Lua55};

fn ok(src: &str, v: LuaVersion) {
    if let Err(e) = parse(src.as_bytes(), v) {
        panic!("expected {src:?} to parse under {v:?}, got: {e}");
    }
}

fn err(src: &str, v: LuaVersion) -> luna::frontend::SyntaxError {
    match parse(src.as_bytes(), v) {
        Ok(_) => panic!("expected {src:?} to fail under {v:?}"),
        Err(e) => e,
    }
}

#[test]
fn version_gates_55_syntax() {
    ok("global <const> *", Lua55);
    ok("global x = 1", Lua55);
    ok("global x <const>, y", Lua55);
    ok("global function f() end", Lua55);
    ok("local <const> a, b = 1, 2", Lua55);
    ok("function f(...t) return t end", Lua55);
    err("global x = 1", Lua54);
    err("global <const> *", Lua54);
    err("local <const> a = 1", Lua54);
    err("function f(...t) end", Lua54);
    // `global` stays a plain name below 5.5
    ok("local global = 1; print(global)", Lua54);
}

#[test]
fn version_gates_54_syntax() {
    ok("local x <const> = 1", Lua54);
    ok("local x <close> = nil", Lua54);
    ok("goto done ::done::", Lua54);
    ok("print(3 // 2, 3 & 1, 3 | 1, 3 ~ 1, 1 << 4, ~0)", Lua54);
    ok(";;;", Lua54);
    err("local x <const> = 1", Lua51);
    err("a = 3 & 1", Lua51);
    err("a = ~0", Lua51);
    err(";", Lua51);
}

#[test]
fn lua51_restrictions() {
    // goto is a plain name in 5.1
    err("goto done ::done::", Lua51);
    // break must end the block in 5.1
    ok("while true do break end", Lua51);
    err("while true do break print() end", Lua51);
    ok("while true do break print() end", Lua54);
    // ambiguous call/new-statement split across lines
    err("f\n(3)", Lua51);
    ok("f\n(3)", Lua55);
    ok("f(3)", Lua51);
}

#[test]
fn assignment_targets() {
    ok("a.b[1].c = nil", Lua55);
    err("(a) = 1", Lua55);
    err("f() = 1", Lua55);
    err("a:b() = 1", Lua55);
    err("2 = 1", Lua55);
}

#[test]
fn error_lines() {
    assert_eq!(err("\n\nx ==", Lua55).line, 3);
    assert_eq!(err("if x then\n", Lua55).line, 2);
    assert_eq!(err("a = [[\n\n", Lua55).line, 3);
    let e = err("if x then\n\n\nelse", Lua55);
    assert!(e.msg.contains("to close 'if' at line 1"), "msg: {}", e.msg);
}

#[test]
fn misc_rejects() {
    err("x =", Lua55);
    err("return return", Lua55);
    err("local 1 = 2", Lua55);
    err("f(,)", Lua55);
    err("a = {", Lua55);
    err("function f( end", Lua55);
    err("a = 1 +", Lua55);
}

#[test]
fn deep_nesting_is_limited() {
    let deep = format!("x = {}0{}", "(".repeat(300), ")".repeat(300));
    let e = err(&deep, Lua55);
    assert!(e.msg.contains("syntax levels"), "msg: {}", e.msg);
    // ...but reasonable depth is fine
    let fine = format!("x = {}0{}", "(".repeat(100), ")".repeat(100));
    ok(&fine, Lua55);
}

#[test]
fn statements_smoke() {
    ok(
        "local t = {1, 2; x = 3, [4] = 5, f(),}\n\
         for i = 1, #t, 2 do print(i) end\n\
         for k, v in pairs(t) do print(k, v) end\n\
         repeat local x = 1 until x\n\
         if a then b() elseif c then d() else e() end\n\
         function a.b.c:m(x, y, ...) return ... end\n\
         local function g() return -#t ^ 2 end\n\
         t.x, t[1] = t[1], t.x\n\
         print 'str' print [[long]] print {tbl = 1}\n\
         do return f(1)(2){3}'4' end",
        Lua55,
    );
    ok("return", Lua55);
    ok("", Lua55);
    ok("#!/usr/bin/env lua\nreturn 1", Lua55);
}
