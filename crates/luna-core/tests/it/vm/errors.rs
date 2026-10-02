//! Error messages: variable info, wording, positions and chunk ids.

use super::*;

#[test]
fn type_error_varinfo() {
    // index / call / arith type errors name the offending operand (getobjname)
    check_error(
        "local x return x.y",
        "attempt to index a nil value (local 'x')",
    );
    check_error("return undefined_glob.y", "(global 'undefined_glob')");
    check_error("local t = {} return t.a.b", "(field 'a')");
    check_error(
        "local f local r = f()",
        "attempt to call a nil value (local 'f')",
    );
    check_error(
        "local r = nope()",
        "attempt to call a nil value (global 'nope')",
    );
    check_error(
        "local n return n + 1",
        "attempt to perform arithmetic on a nil value (local 'n')",
    );
    // an upvalue operand is named too
    check_error(
        "local up local function g() return up.x end return g()",
        "(upvalue 'up')",
    );
}

#[test]
fn error_object_edges() {
    // error() / error(nil): a nil error object becomes "<no error object>"
    check_str(
        "local ok, msg = pcall(function() error() end) return msg",
        b"<no error object>",
    );
    check_str(
        "local ok, msg = pcall(function() error(nil) end) return msg",
        b"<no error object>",
    );
    // a non-nil error object is preserved (string with position prefix)
    check_bool(
        "local ok, msg = pcall(function() error('boom', 0) end) return msg == 'boom'",
        true,
    );
    // tostring/tonumber require an argument (luaL_checkany)
    check_error(
        "return tostring()",
        "bad argument #1 to 'tostring' (value expected)",
    );
    check_error(
        "return tonumber()",
        "bad argument #1 to 'tonumber' (value expected)",
    );
    check_str("return tostring(1)", b"1");
}

#[test]
fn comparison_and_for_error_wording() {
    // PUC luaG_ordererror: matching types report "two X values"
    check_error(
        "return print < print",
        "attempt to compare two function values",
    );
    check_error("return {} < {}", "attempt to compare two table values");
    check_error("return 1 < 'x'", "attempt to compare number with string");
    // PUC luaG_forerror: "bad 'for' <what> (number expected, got <type>)"
    check_error(
        "for i = 1, 'x', 10 do end",
        "bad 'for' limit (number expected, got string)",
    );
    check_error(
        "for i = 1, 10, print do end",
        "bad 'for' step (number expected, got function)",
    );
}

#[test]
fn parser_near_token_and_goto_lines() {
    // <eof> is the only unquoted near-token (PUC luaX_token2str)
    check_compile_error("local a = {", "near <eof>");
    check_compile_error("local a = (1", "near <eof>");
    // a normal token is single-quoted
    check_compile_error("local a = 1 +", "near <eof>");
    // goto/label diagnostics carry the relevant source line (PUC)
    check_compile_error("::A:: a = 1 ::A::", "already defined on line 1");
    check_compile_error(
        "goto A do ::A:: end",
        "no visible label 'A' for <goto> at line 1",
    );
}

#[test]
fn lexer_string_escape_near_tokens() {
    // PUC near-token = the lex buffer: the string read so far, opening
    // delimiter included; decimal-too-large includes the char after the
    // digits, utf8-too-large stops at the digit.
    check_compile_error(r#"return "\999""#, r#"near '"\999"'"#);
    check_compile_error(
        r#"return "abc\u{100000000}""#,
        r#"UTF-8 value too large near '"abc\u{100000000'"#,
    );
    check_compile_error(r#"return "abc\u{11r""#, r#"missing '}' near '"abc\u{11r'"#);
    check_compile_error(r#"return "abc\u""#, r#"missing '{' near '"abc\u"'"#);
    // unfinished string reports the <eof> token
    check_compile_error("return 'alo", "unfinished string near <eof>");
}

#[test]
fn error_message_fidelity() {
    // errors.lua regressions for PUC-faithful error wording.
    // A non-callable metamethod names the dispatching event.
    check_error(
        "local a = setmetatable({}, {__add = 34}); local _ = a + 1",
        "metamethod 'add'",
    );
    // A tail call to a nil field keeps the field name (frame popped early).
    check_error("local a = {}; return a.bbbb(3)", "field 'bbbb'");
    // __name (luaT_objtypename) drives type names in arithmetic/compare errors.
    check_error(
        "local x = setmetatable({}, {__name = 'My Type'}); local _ = x + 1",
        "on a My Type value",
    );
    check_error(
        "local x = setmetatable({}, {__name = 'My Type'}); local _ = x < x",
        "two My Type values",
    );
    // A field literally named `_ENV` is a field, not a global.
    check_error("local a = {_ENV = {}}; local _ = a._ENV.x + 1", "field 'x'");
    // collectgarbage rejects unknown options (luaL_checkoption).
    check_error("collectgarbage('nooption')", "invalid option");
    // A C function called as a method rewrites the self-argument error.
    check_error(
        "local t = setmetatable({}, {__index = string}); t:rep(2)",
        "calling 'rep' on bad self",
    );
    // luaL_optinteger position args report a proper argument error.
    check_error("return string.sub('a', {})", "number expected, got table");
    check_error("return string.sub('a', {})", "#2");
    // A stripped chunk (no source) reports the "?:?:" position prefix.
    check_error(
        "local f = assert(load(string.dump(function () return nil + 1 end, true))); f()",
        "?:?:",
    );
}

#[test]
fn pushglobalfuncname_qualifies_nested_native_arg_error() {
    // errors.lua:381: `table.sort({1,2,3}, table.sort)` — the inner sort
    // (called as a comparator from the outer sort's native) detects bad
    // arg #1 (a number). PUC's `pushglobalfuncname` walks package.loaded
    // and qualifies the running function's name as `'table.sort'`.
    check_error("table.sort({1,2,3}, table.sort)", "'table.sort'");
    // errors.lua:382: `string.gsub('s', 's', setmetatable)` — the inner
    // setmetatable is invoked from gsub's native replacement loop; PUC
    // finds `_G.setmetatable` and strips the `_G.` prefix.
    check_error("string.gsub('s', 's', setmetatable)", "'setmetatable'");
    // A direct (non-nested) native arg error keeps the bare name. (The
    // array must hold two elements: 5.3+ checks the comparator only then.)
    check_error("table.sort({2, 1}, 7)", "'sort'");
}

#[test]
fn syntax_error_source_uses_chunkid() {
    // errors.lua:402-416: a syntax error's source prefix is rendered via
    // luaO_chunkid (LUA_IDSIZE=60). `@file` is tail-truncated behind "...";
    // `=name` is head-truncated; a raw string source is wrapped as
    // `[string "first line..."]`. The prefix before the first `:` is ≤59.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let name_at = format!("@{}", "x".repeat(70));
    let src = format!("return load('x', '{name_at}')");
    let r = vm.eval(&src).expect("load itself succeeds");
    // `load` with a bad source (here: parse-time error) returns `(nil, msg)`;
    // the chunk's `return` surfaces both values.
    let msg = r.into_iter().nth(1).expect("(nil, msg)");
    if let Value::Str(s) = msg {
        let bytes = s.as_bytes();
        assert!(
            bytes.starts_with(b"..."),
            "expected '...' truncation, got {:?}",
            String::from_utf8_lossy(bytes)
        );
        let prefix = bytes.split(|b| *b == b':').next().unwrap();
        assert!(prefix.len() <= 59, "prefix len {} > 59", prefix.len());
    } else {
        panic!("load did not return a string error");
    }
}

#[test]
fn getobjname_global_via_gettable() {
    // A global whose key constant index exceeds the GETFIELD operand limit is
    // compiled as GETTABLE; the operand must still name "global 'bbb'".
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("aaa = x{i}; "));
    }
    src.push_str("local _ = bbb + 1");
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(&src) {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => assert!(
            vm.error_text(&e).contains("global 'bbb'"),
            "unexpected: {}",
            vm.error_text(&e)
        ),
    }
}
