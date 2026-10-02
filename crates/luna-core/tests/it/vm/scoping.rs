//! goto, const attribs and 5.5 global declarations.

use super::*;

#[test]
fn goto_and_labels() {
    // forward goto
    check_int("do goto done end ::done:: return 1", 1);
    check_int("local x = 1 goto skip x = 99 ::skip:: return x", 1);
    // backward goto (loop)
    check_int(
        "local n = 0 ::top:: n = n + 1 if n < 5 then goto top end return n",
        5,
    );
    // continue idiom: trailing label may skip over locals
    check_int(
        "local s = 0 for i = 1, 5 do if i % 2 == 0 then goto continue end \
         local double = i * 2 s = s + double ::continue:: end return s",
        18,
    );
    // goto out of nested blocks
    check_int("do do goto out end end ::out:: return 7", 7);
    // errors
    check_compile_error("goto nowhere", "no visible label 'nowhere'");
    check_compile_error(
        "goto later local x = 1 ::later:: return x",
        "jumps into the scope",
    );
    check_compile_error("::dup:: ::dup::", "already defined");
    // a label conflicts with a visible label in an enclosing block
    check_compile_error("::l1:: do ::l1:: end", "label 'l1' already defined");
    // a label inside a nested block is not visible to an outer goto
    check_compile_error("goto l1 do ::l1:: end", "no visible label 'l1'");
    // 5.5 scope-jump wording carries no "local"
    check_compile_error(
        "goto l1 local aa ::l1:: return aa",
        "jumps into the scope of 'aa'",
    );
    // goto leaving a block with captured locals closes them
    check_int(
        "local fs = {} local i = 1 ::top:: do local v = i fs[i] = function() return v end end \
         i = i + 1 if i <= 2 then goto top end return fs[1]() * 10 + fs[2]()",
        12,
    );
}

#[test]
fn const_attribs() {
    check_int("local x <const> = 41 return x + 1", 42);
    check_compile_error(
        "local x <const> = 1 x = 2",
        "attempt to assign to const variable 'x'",
    );
    // 5.5 collective attrib on locals
    check_compile_error(
        "local <const> a, b = 1, 2 b = 3",
        "attempt to assign to const variable 'b'",
    );
    // for-loop control variables are const in 5.5
    check_compile_error(
        "for i = 1, 3 do i = 5 end",
        "attempt to assign to const variable 'i'",
    );
    check_compile_error(
        "for k, v in pairs({}) do k = 1 end",
        "attempt to assign to const variable 'k'",
    );
    // non-control generic-for variables stay writable
    check_int(
        "for k, v in pairs({x = 1}) do v = 7 return v end return 0",
        7,
    );
    // assigning to a const captured as an upvalue in a nested function
    check_compile_error(
        "local z <const> = 1 function foo() return function() z = 2 end end",
        "attempt to assign to const variable 'z'",
    );
    // function statement assigning to a const name
    check_compile_error(
        "local foo <const> = 10 function foo() end",
        "attempt to assign to const variable 'foo'",
    );
}

#[test]
fn global_declarations_55() {
    // explicit declarations: declared names work, undeclared error
    check_int("global x = 5 return x + 1", 6);
    check_compile_error("global x = 1 return y", "variable 'y' not declared");
    check_compile_error("global x = 1 y = 2", "variable 'y' not declared");
    // collective global * restores default-style access
    check_int("global x = 1 global * y = 2 return x + y", 3);
    // global <const> *: reads fine, writes to undeclared error
    check_int(
        "global <const> * return type(print) == 'function' and 1 or 0",
        1,
    );
    check_compile_error(
        "global <const> * y = 2",
        "attempt to assign to const variable 'y'",
    );
    // explicitly declared names stay writable under a const collective
    check_int("global <const> * global n n = 41 return n + 1", 42);
    // const global declaration: initializer allowed, later writes error
    check_compile_error(
        "global z <const> = 1 z = 2",
        "attempt to assign to const variable 'z'",
    );
    // declarations are block-scoped: outside the block, default returns
    check_int("do global x x = 1 end y = 2 return y", 2);
    // global function declares its name
    check_int("global function gf() return 21 end return gf() * 2", 42);
    // locals are unaffected by strict mode
    check_int("global g local a = 3 g = a return g", 3);
    // _ENV bypass still works (declarations are purely syntactic)
    check_int("global <const> * _ENV.bypass = 9 return _ENV.bypass", 9);
}

#[test]
fn global_declarations_55_edges() {
    // `global` is a contextual keyword: an ordinary identifier unless it leads
    // a declaration (followed by a name / '*' / function / attribute).
    check_int("global = 1; return global", 1);
    check_int("local global = 41; return global + 1", 42);

    // explicit declaration + strict default once any global is declared
    check_compile_error("global none; X = 1", "variable 'X'");
    check_compile_error(
        "global none; local function f() XXX = 1 end",
        "variable 'XXX'",
    );

    // a `global *` collective re-opens implicit globals
    check_int("global *; Y = 7; return _ENV.Y", 7);

    // const globals are read-only after declaration, writable as the defining
    // initializer
    check_compile_error(
        "global<const> foo; function foo() end",
        "assign to const variable 'foo'",
    );
    check_int("global<const> a = 5; return _ENV.a", 5);

    // close attribute is rejected on globals with the 5.5 wording
    check_compile_error("global X<close>", "cannot be to-be-closed");
    check_compile_error("global <close> *", "cannot be to-be-closed");

    // `_ENV` pulled into a global declaration makes every global access error
    check_compile_error("global _ENV, a; a = 10", "variable 'a'");

    // an inner `global X` shadows an enclosing local X for that scope only
    check_int(
        "local X = 10; do global X; X = 20 end; return X * 1000 + _ENV.X",
        10020,
    );

    // an initializer reads the enclosing scope, not the global being defined
    check_int(
        "local a, b = 100, 200; do global a, b = a, b end; return _ENV.a + _ENV.b",
        300,
    );

    // a defining write to an already-existing global is a runtime error
    check_error(
        "_ENV.dup = 1; global dup = 2",
        "global 'dup' already defined",
    );
    check_error(
        "_ENV.fdup = 1; global function fdup() end",
        "global 'fdup' already defined",
    );
}

#[test]
fn goto_scope_over_declarations() {
    // a goto cannot jump over a local declaration into its scope
    check_compile_error("goto l1; local aa ::l1:: ::l2:: return 0", "scope of 'aa'");
    // ...nor over a `global *` collective marker ('*' in the wording)
    check_compile_error("goto l2; global *; ::l1:: ::l2:: return 0", "scope of '*'");
    // repeat-until keeps body locals alive through the condition, so a goto to
    // a trailing label lands in their scope
    check_compile_error(
        "repeat if x then goto cont end local xuxu = 10 ::cont:: until xuxu < 1",
        "scope of 'xuxu'",
    );
}

#[test]
fn parse_time_local_var_limit() {
    // errors.lua :775: more than MAXVARS=200 locals inside a function raises
    // the limit error at PARSE time so a later structural error (a missing
    // `end`) doesn't steal the spotlight. The "in function at line N" suffix
    // names the function's defining line.
    let s = std::iter::once("function foo ()\n  local ".to_string())
        .chain((1..=200).map(|j| format!("a{j}, ")))
        .chain(std::iter::once("b\nend".to_string()))
        .collect::<String>();
    let mut vm = Vm::new(LuaVersion::Lua55);
    let err = vm.load(s.as_bytes(), b"=t").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("too many local variables"), "msg = {msg:?}");
    assert!(msg.contains("function at line 1"), "msg = {msg:?}");
    // Per-block scoping: the same names re-declared after each do…end stay
    // under the cap, so this large chunk must compile cleanly (locals.lua
    // exercises this pattern heavily).
    let s = (0..50)
        .map(|_| "do local a,b,c,d,e,f,g,h end ".to_string())
        .collect::<String>();
    let _ = vm.load(s.as_bytes(), b"=t").expect("scoped locals reset");
}
