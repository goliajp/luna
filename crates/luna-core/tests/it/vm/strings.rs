//! The string library, patterns, string.format and utf8.

use super::*;

#[test]
fn string_core() {
    check_int("return string.len('hello')", 5);
    check_int("return ('hello'):len()", 5); // method syntax via string metatable
    check_str("return ('hello'):sub(2, 4)", b"ell");
    check_str("return ('hello'):sub(-3)", b"llo");
    check_str("return ('hello'):sub(2)", b"ello");
    check_str("return ('hello'):sub(4, 2)", b"");
    check_str("return ('hello'):sub(-100, 100)", b"hello");
    check_str("return ('aBc'):upper()", b"ABC");
    check_str("return ('aBc'):lower()", b"abc");
    check_str("return ('ab'):rep(3)", b"ababab");
    check_str("return ('ab'):rep(3, '-')", b"ab-ab-ab");
    check_str("return ('ab'):rep(0)", b"");
    check_str("return ('abc'):reverse()", b"cba");
    check_int("return ('A'):byte()", 65);
    check_int("return select('#', ('abc'):byte(1, 3))", 3);
    check_str("return string.char(104, 105)", b"hi");
    check_error("return string.char(300)", "value out of range");
    // numbers coerce in string functions
    check_int("return string.len(123)", 3);
}

#[test]
fn string_find_and_match() {
    check_int("return (string.find('hello', 'll'))", 3);
    check_int("return select(2, string.find('hello', 'll'))", 4);
    check_bool("return string.find('hello', 'xyz') == nil", true);
    check_int("return (string.find('hello', 'l+'))", 3);
    check_int("return (string.find('a.b', '.', 1, true))", 2); // plain
    check_int("return (string.find('hello', 'l', -2))", 4); // negative init
    check_str("return (string.match('key=val', '(%w+)=(%w+)'))", b"key");
    check_str(
        "return select(2, string.match('key=val', '(%w+)=(%w+)'))",
        b"val",
    );
    check_str("return string.match('hello 42!', '%d+')", b"42");
    check_bool("return string.match('abc', '%d') == nil", true);
    check_int("return string.match('abc', '()b')", 2); // position capture
    check_str("return string.match('  trim  ', '^%s*(.-)%s*$')", b"trim");
    check_error("return string.match('x', '%')", "malformed pattern");
}

#[test]
fn string_gmatch() {
    check_int(
        "local n = 0 for w in ('one two three'):gmatch('%a+') do n = n + 1 end return n",
        3,
    );
    check_str(
        "local t = {} for k, v in ('a=1,b=2'):gmatch('(%w+)=(%w+)') do t[#t+1] = k .. v end \
         return table.concat(t, ' ')",
        b"a1 b2",
    );
    // standalone iterator calls work (closure state, no generic for)
    check_str(
        "local it = ('x y'):gmatch('%a') local a = it() local b = it() return a .. b",
        b"xy",
    );
    // empty matches make progress
    check_int(
        "local n = 0 for _ in ('abc'):gmatch('x*') do n = n + 1 end return n",
        4,
    );
}

#[test]
fn string_gsub() {
    check_str("return (('hello world'):gsub('o', '0'))", b"hell0 w0rld");
    check_int("return select(2, ('hello'):gsub('l', 'L'))", 2);
    check_str("return (('hello'):gsub('l', 'L', 1))", b"heLlo");
    check_str("return (('abc'):gsub('(%a)', '%1%1'))", b"aabbcc");
    check_str(
        "return (('key=val'):gsub('(%w+)=(%w+)', '%2=%1'))",
        b"val=key",
    );
    check_str("return (('ab'):gsub('b', '100%%'))", b"a100%");
    // table replacement
    check_str(
        "return (('$name is $age'):gsub('%$(%w+)', {name = 'lua', age = 30}))",
        b"lua is 30",
    );
    // function replacement; false/nil keeps the original
    check_str(
        "return (('1 2 3'):gsub('%d', function(d) return tonumber(d) * 2 end))",
        b"2 4 6",
    );
    check_str(
        "return (('keep drop'):gsub('%a+', function(w) if w == 'drop' then return 'X' end end))",
        b"keep X",
    );
    // empty pattern progress
    check_str("return (('ab'):gsub('', '-'))", b"-a-b-");
    check_error("return ('x'):gsub('x', '%9')", "invalid capture index");
    check_str("return (('x'):gsub('x', {}))", b"x"); // table lookup nil keeps original
    // 5.3.3 empty-match semantics: no double replacement after a non-empty one
    check_str("return (('a b cd'):gsub(' *', '-'))", b"-a-b-c-d-");
    check_int("return select(2, ('a b cd'):gsub(' *', '-'))", 5);
    // table replacement honours __index
    check_str(
        "local t = setmetatable({}, {__index = function(_, k) return k:upper() end}) \
         return (('a bb'):gsub('%a+', t))",
        b"A BB",
    );
    // no-change reuse: the count still reflects matches even when nothing changed
    check_int("return select(2, ('aaa'):gsub('.', {}))", 3);
}

#[test]
fn string_gmatch_init_and_empty() {
    // 1-based init parameter (5.4)
    check_int(
        "local s = 0 for k in ('10 20 30'):gmatch('%d+', 3) do s = s + tonumber(k) end return s",
        50,
    );
    // negative init counts from the end
    check_int(
        "local s = 0 for k in ('11 21 31'):gmatch('%d+', -2) do s = s + tonumber(k) end return s",
        31,
    );
    // position-capture empty matches advance cleanly (PUC lastmatch rule)
    check_str(
        "local r = '' local i = 1 local sub = 'a b' \
         for p, e in sub:gmatch('()%s*()') do r = r .. sub:sub(i, p - 1) .. '-' i = e end \
         return r",
        b"-a-b-",
    );
}

#[test]
fn pattern_classes_balanced_frontier() {
    check_str("return string.match('foo (bar) baz', '%b()')", b"(bar)");
    check_str("return string.match('THE quick', '%f[%l]%a+')", b"quick");
    check_str("return string.match('abc123', '%a+')", b"abc");
    check_str("return string.match('abc123', '%A+')", b"123"); // complement... wait %A = non-alpha → 123
    check_str("return string.match('a-b', '%p')", b"-");
    check_str("return string.match('x\\ty', '%s')", b"\t");
    check_str("return string.match('abcabc', '(a%w+)%1')", b"abc"); // backref... wait (a%w+) greedy
    check_str("return string.match('[x]', '%[(%a)%]')", b"x");
}

#[test]
fn string_format() {
    check_str("return string.format('%d', 42)", b"42");
    check_str("return string.format('%d', -42)", b"-42");
    check_str("return string.format('%5d', 42)", b"   42");
    check_str("return string.format('%-5d|', 42)", b"42   |");
    check_str("return string.format('%05d', 42)", b"00042");
    check_str("return string.format('%+d %+d', 5, -5)", b"+5 -5");
    check_str("return string.format('%x', 255)", b"ff");
    check_str("return string.format('%X', 255)", b"FF");
    check_str("return string.format('%#x', 255)", b"0xff");
    check_str("return string.format('%o', 8)", b"10");
    check_str("return string.format('%x', -1)", b"ffffffffffffffff");
    check_str("return string.format('%c%c', 104, 105)", b"hi");
    check_str("return string.format('%s=%s', 'a', 1)", b"a=1");
    check_str("return string.format('%10s|', 'hi')", b"        hi|");
    check_str("return string.format('%-10s|', 'hi')", b"hi        |");
    check_str("return string.format('%.3s', 'hello')", b"hel");
    check_str("return string.format('%f', 1.5)", b"1.500000");
    check_str("return string.format('%.2f', 3.14159)", b"3.14");
    check_str("return string.format('%.0f', 2.5)", b"2");
    check_str("return string.format('%e', 1500.0)", b"1.500000e+03");
    check_str("return string.format('%.2E', 0.0001)", b"1.00E-04");
    check_str("return string.format('%g', 100000.0)", b"100000");
    check_str("return string.format('%g', 1e+20)", b"1e+20");
    check_str("return string.format('%g', 0.0001)", b"0.0001");
    check_str("return string.format('%g', 0.00001)", b"1e-05");
    check_str("return string.format('%.3g', 3.14159)", b"3.14");
    check_str("return string.format('%g', 2.0)", b"2");
    check_str("return string.format('%a', 1.0)", b"0x1p+0");
    check_str("return string.format('%a', 0.5)", b"0x1p-1");
    check_str("return string.format('%a', 3.0)", b"0x1.8p+1");
    check_str("return string.format('%d%%', 99)", b"99%");
    // %q round-trips. PUC addquoted escapes a newline as backslash + a real
    // newline (not `\n`), so it reads back as the same string.
    check_str(
        "return string.format('%q', 'a\\nb\"c\\\\d')",
        b"\"a\\\nb\\\"c\\\\d\"",
    );
    // %q of math.mininteger uses a hex literal (decimal would reparse as float)
    check_str(
        "return string.format('%q', math.mininteger)",
        b"0x8000000000000000",
    );
    check_str("return string.format('%q', 7)", b"7");
    check_bool(
        "return load('return ' .. string.format('%q', 'x\\0y'))() == 'x\\0y'",
        true,
    );
    check_str("return string.format('%q', 0/0)", b"(0/0)");
    check_str("return string.format('%q', 2.0)", b"0x1p+1");
    // tostring path honors __tostring in %s
    check_str(
        "local t = setmetatable({}, {__tostring = function() return 'T' end}) \
         return string.format('[%s]', t)",
        b"[T]",
    );
    // errors
    check_error(
        "return string.format('%d', 1.5)",
        "no integer representation",
    );
    check_error("return string.format('%d')", "no value");
    check_error("return string.format('%k', 1)", "invalid conversion");
}

#[test]
fn utf8_library() {
    check_str("return utf8.char(72, 105)", b"Hi");
    check_str("return utf8.char(0x4F60, 0x597D)", "你好".as_bytes());
    check_int("return utf8.len('héllo')", 5);
    check_int("return utf8.len('你好')", 2);
    check_int("return (utf8.codepoint('你好'))", 0x4F60);
    check_int("return select(2, utf8.codepoint('你好', 1, -1))", 0x597D);
    check_int("return (utf8.offset('你好', 2))", 4);
    // 5.5: offset also returns the character's final byte position
    check_int("return select(2, utf8.offset('你好', 1))", 3);
    check_int("return (utf8.offset('你好x', -1))", 7);
    check_int(
        "local n = 0 for p, c in utf8.codes('a你b') do n = n + 1 end return n",
        3,
    );
    check_int(
        "local last for p in utf8.codes('a你b') do last = p end return last",
        5,
    );
    // invalid sequences
    check_bool("return utf8.len('\\xFF') == nil", true);
    check_int("return select(2, utf8.len('a\\xFFb'))", 2);
    check_error("return utf8.codepoint('\\x80')", "invalid UTF-8 code");
    check_bool("return utf8.charpattern ~= nil", true);
    // offset(s, 0, i): start and end byte positions of the char containing i
    check_int("return (utf8.offset('a你b', 0, 3))", 2);
    check_int("return select(2, utf8.offset('a你b', 0, 3))", 4);
    // bounds errors (position out of bounds / continuation byte)
    check_error("return utf8.offset('abc', 1, 5)", "position out of bounds");
    check_error("return utf8.offset('', 1, 2)", "position out of bounds");
    check_error("return utf8.offset('\\x80', 1)", "continuation byte");
    check_error("return utf8.len('abc', 0, 2)", "out of bounds");
    check_error("return utf8.len('abc', 1, 4)", "out of bounds");
}

#[test]
fn string_format_modifiers() {
    // strings.lua regressions for string.format spec handling.
    // %s with a modifier rejects embedded zeros (PUC "string contains zeros").
    check_error(
        "return string.format('%10s', '\\0')",
        "string contains zeros",
    );
    // %a honours precision (round to N hex digits, ties-to-even).
    check_str("return string.format('%+.2A', 12)", b"+0X1.80P+3");
    check_str("return string.format('%.4A', -12)", b"-0X1.8000P+3");
    // `#` forces a radix point; the `0` flag zero-pads floats even with a
    // precision (unlike integer conversions).
    check_str("return string.format('%+#014.0f', 100)", b"+000000000100.");
    // per-conversion flag/width validation (PUC checkformat wording).
    check_error("return string.format('%100.3d', 10)", "invalid conversion");
    check_error("return string.format('%#i', 10)", "invalid conversion");
    check_error("return string.format('%010c', 10)", "invalid conversion");
    check_error("return string.format('%F', 10)", "invalid conversion");
    // over-long spec → "too long" (not "invalid conversion").
    check_error(
        "return string.format('%'..string.rep('0',600)..'d', 10)",
        "too long",
    );
}

#[test]
fn pattern_backref_zero_is_invalid() {
    // `%0` is not a valid back-reference: it must error, not panic on the
    // `d - b'1'` subtraction (debug-build overflow). Regression for pm.lua.
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval("return string.match('abc', '%0')") {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("invalid capture index"),
                "unexpected error: {msg}"
            );
        }
    }
}

#[test]
fn string_constants_are_interned_per_chunk() {
    // identical literals — even long (>40 bytes) ones, across nested functions —
    // share one object, so their %p addresses compare equal.
    let long = "0123456789012345678901234567890123456789012345"; // 46 bytes
    check_int(
        &format!(
            "local a <const> = {long:?} \
             local function f() return {long:?} end \
             return (string.format('%p', a) == string.format('%p', f())) and 1 or 0"
        ),
        1,
    );
    // a `#` chunk is the length operator, not a shebang (string load never strips)
    check_compile_error("#x = 1", "unexpected symbol");
}
