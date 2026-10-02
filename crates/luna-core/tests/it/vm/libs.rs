//! math, table, base, load, warn and os libraries.

use super::*;

#[test]
fn math_library() {
    check_int("return math.floor(2.7)", 2);
    check_int("return math.floor(-2.7)", -3);
    check_int("return math.ceil(2.1)", 3);
    check_int("return math.abs(-5)", 5);
    check_float("return math.abs(-5.5)", 5.5);
    check_int("return math.max(3, 1, 4, 1, 5)", 5);
    check_int("return math.min(3, 1, 4, 1, 5)", 1);
    check_float("return math.sqrt(16)", 4.0);
    check_float("return math.huge", f64::INFINITY);
    check_int("return math.maxinteger", i64::MAX);
    check_int("return math.mininteger", i64::MIN);
    check_str("return math.type(1)", b"integer");
    check_str("return math.type(1.0)", b"float");
    check_bool("return math.type('x') == nil", true);
    check_int("return math.tointeger(3.0)", 3);
    check_bool("return math.tointeger(3.5) == nil", true);
    check_int("return math.fmod(7, 3)", 1);
    check_int("return math.fmod(-7, 3)", -1); // fmod truncates, % floors
    check_error("return math.fmod(1, 0)", "zero");
    check_bool("return math.ult(-1, 1)", false); // -1 is huge unsigned
    check_float("return math.log(8, 2)", 3.0);
    // math.modf integer part returns Integer subtype
    // when it fits i64 (matches PUC 5.4/5.5 pushnumint fast path).
    let v = eval("local ip, fp = math.modf(3.7) return ip");
    assert!(matches!(v[0], Value::Int(3)));
    // random: determinism after seeding, ranges respected
    check_bool(
        "math.randomseed(42) local a = math.random() math.randomseed(42) \
         return a == math.random()",
        true,
    );
    check_bool(
        "math.randomseed(7) for i = 1, 100 do local r = math.random(3, 9) \
         if r < 3 or r > 9 then return false end end return true",
        true,
    );
    check_error("return math.random(5, 2)", "interval is empty");
    check_error("return math.random(1, 2, 3)", "wrong number of arguments");
    // xoshiro256** conformance: PUC's exact sequence after seed 1007
    check_int(
        "math.randomseed(1007) return math.random(0)",
        0x7a7040a5a323c9d6u64 as i64,
    );
    // deg/rad/frexp/ldexp
    check_bool("return math.deg(math.pi) == 180.0", true);
    check_bool("return math.rad(180) == math.pi", true);
    check_bool(
        "local m, e = math.frexp(8.0) return m == 0.5 and e == 4",
        true,
    );
    check_bool("return math.ldexp(0.5, 4) == 8.0", true);
    // float modulo keeps fmod's sign correction for tiny denormals (no m*y
    // underflow): (-1).0 % 2.0 floors toward the divisor
    check_float("return (-1.0) % 2.0", 1.0);
    // -0.0 survives the LoadF fast path (1/-0.0 == -inf)
    check_bool("local z <const> = -0.0 return 1/z < 0", true);
    // bitwise on a non-integer field names the operand
    check_error("return math.huge << 1", "field 'huge'");
}

#[test]
fn table_library() {
    check_int("local t = {1, 2, 3} table.insert(t, 4) return t[4] + #t", 8);
    check_int(
        "local t = {1, 3} table.insert(t, 2, 2) return t[1] * 100 + t[2] * 10 + t[3]",
        123,
    );
    check_error("table.insert({}, 5, 1)", "position out of bounds");
    check_error("table.insert({}, 2, 3, 4)", "wrong number of arguments");
    // table.insert/remove use luaL_len: a non-integer __len is an error
    check_error(
        "local t = setmetatable({}, {__len = function() return 'abc' end}) table.insert(t, 1)",
        "object length is not an integer",
    );
    // table.create range/overflow checks (both args)
    check_error("table.create(0, 1 << 31)", "out of range");
    check_error("table.create(0, (1 << 31) - 1)", "table overflow");
    // table.unpack with a full-integer range must error, not hang
    check_error(
        "table.unpack({}, math.mininteger, math.maxinteger)",
        "too many results",
    );
    check_int(
        "local t = {1, 2, 3} local v = table.remove(t) return v * 10 + #t",
        32,
    );
    check_int(
        "local t = {1, 2, 3} local v = table.remove(t, 1) return v * 10 + t[1]",
        12,
    );
    check_str("return table.concat({1, 'b', 2.5}, '-')", b"1-b-2.5");
    check_str("return table.concat({}, 'x')", b"");
    check_str("return table.concat({9, 8, 7}, '', 2, 3)", b"87");
    check_error("table.concat({{}})", "invalid value");
    check_int(
        "local a, b, c = table.unpack({7, 8, 9}) return a * 100 + b * 10 + c",
        789,
    );
    check_int("return (table.unpack({1, 2, 3}, 2))", 2);
    check_int("return select('#', table.unpack({1, 2, 3}))", 3);
    check_int("local p = table.pack(4, 5, 6) return p.n * 100 + p[3]", 306);
    check_int(
        "local t = {1, 2, 3, 4, 5} table.move(t, 1, 3, 3) \
         return t[3] * 100 + t[4] * 10 + t[5]",
        123,
    );
    check_int(
        "local d = table.move({7, 8}, 1, 2, 1, {}) return d[1] * 10 + d[2]",
        78,
    );
    check_int("return #table.create(16)", 0);
    // sort: default order, comparator, strings, invalid order caught
    check_str(
        "local t = {3, 1, 4, 1, 5, 9, 2, 6} table.sort(t) return table.concat(t, '')",
        b"11234569",
    );
    check_str(
        "local t = {3, 1, 4, 1, 5} table.sort(t, function(a, b) return a > b end) \
         return table.concat(t, '')",
        b"54311",
    );
    check_str(
        "local t = {'pear', 'apple', 'fig'} table.sort(t) return table.concat(t, ',')",
        b"apple,fig,pear",
    );
    check_bool(
        "local t = {} for i = 1, 200 do t[i] = (i * 37) % 101 end table.sort(t) \
         for i = 2, 200 do if t[i - 1] > t[i] then return false end end return true",
        true,
    );
    check_error(
        "local t = {} for i = 1, 64 do t[i] = i end \
         table.sort(t, function() return true end)",
        "invalid order function",
    );
}

#[test]
fn base_additions() {
    check_int("return tonumber('42')", 42);
    check_float("return tonumber('2.5')", 2.5);
    check_int("return tonumber('0x10')", 16);
    check_bool("return tonumber('zz') == nil", true);
    check_bool("return tonumber({}) == nil", true);
    check_int("return tonumber('ff', 16)", 255);
    check_int("return tonumber('111', 2)", 7);
    check_int("return tonumber('-z', 36)", -35);
    check_bool("return tonumber('12', 2) == nil", true);
    check_error("return tonumber('1', 99)", "base out of range");
    // load
    check_int("local f = load('return 1 + 1') return f()", 2);
    check_int("local f = load('return ...', 'chunk') return f(9)", 9);
    check_bool(
        "local f, e = load('syntax ! error') return f == nil and type(e) == 'string'",
        true,
    );
    // load with custom env
    check_int(
        "local env = {x = 5} local f = load('return x', 'c', 't', env) return f()",
        5,
    );
    // collectgarbage
    check_bool("return collectgarbage('count') > 0", true);
    check_int("return collectgarbage()", 0);
    check_bool("return pairs({}) == next", true);
}

#[test]
fn table_move_and_sort_guards() {
    // table.move honours __index (read) and __newindex (write)
    check_str(
        "local src = setmetatable({}, {__index = function(_, k) return ('%d'):format(k) end}) \
         local dst = table.move(src, 1, 3, 1, {}) return dst[1] .. dst[2] .. dst[3]",
        b"123",
    );
    // range/overflow guards instead of looping
    check_error(
        "table.move({}, 0, math.maxinteger, 1)",
        "too many elements to move",
    );
    check_error(
        "table.move({}, 1, 2, math.maxinteger)",
        "destination wrap around",
    );
    // table.sort honours __len and rejects a too-big array
    check_error(
        "table.sort(setmetatable({}, {__len = function() return math.maxinteger end}))",
        "array too big",
    );
    // an invalid order function is detected even for small arrays
    check_error(
        "table.sort({1,2,3,4}, function() return true end)",
        "invalid order function",
    );
    // a valid sort still works
    check_int(
        "local t = {3,1,2,5,4} table.sort(t) return t[1]*10000+t[2]*1000+t[3]*100+t[4]*10+t[5]",
        12345,
    );
}

#[test]
fn table_concat_at_max_index() {
    // table.concat must not overflow `j + 1` when the range ends at maxi.
    // Regression for strings.lua:413.
    check_str(
        "return table.concat({[math.maxinteger] = 'alo'}, 'x', math.maxinteger, math.maxinteger)",
        b"alo",
    );
}

#[test]
fn stdlibs_registered_in_package_loaded() {
    // every standard library must appear in package.loaded — nextvar.lua's
    // "clear globals" test deletes any global not present there, so a missing
    // entry (coroutine was missing) gets the library wiped mid-run.
    check_bool(
        "for _, n in ipairs{'string','table','math','os','io','utf8','debug','coroutine'} do \
           if package.loaded[n] ~= _G[n] then return false end \
         end \
         return true",
        true,
    );
}

#[test]
fn load_reader_and_mode() {
    // function-reader form: pieces are concatenated until nil
    check_int(
        "local parts = {'return ', '1 + ', '2'} local i = 0 \
         local f = load(function() i = i + 1 return parts[i] end) return f()",
        3,
    );
    // a reader error is a soft failure (nil, msg)
    check_bool("return (load(function() error('boom') end)) == nil", true);
    // a reader returning a non-string fails softly too
    check_bool("return (load(function() return true end)) == nil", true);
    // mode 'b' rejects a text chunk
    check_str(
        "local _, m = load('return 1', 'c', 'b') return m",
        b"attempt to load a text chunk (mode is 'b')",
    );
    // a dumped chunk round-trips through the reader form under mode 'b'
    check_int(
        "local d = string.dump(load('return 6*7')) return load(function() local s=d d=nil return s end, 'c', 'b')()",
        42,
    );
}

#[test]
fn load_seeds_globals_only_when_one_upvalue() {
    // PUC `lua_load` writes the globals table into the loaded closure's
    // first upvalue cell *only* when the closure has exactly one upvalue
    // (the main-chunk `_ENV` case). A dumped non-main function with
    // multiple upvalues keeps every cell at nil — 5.2 calls.lua :293's
    // `assert(x() == nil)` reads the dumped `a` upvalue and must see nil
    // rather than the globals table leaking in.
    check_bool(
        "local a, b = 20, 30 \
         local d = string.dump(function (set) \
             if set == 'set' then a = 10+b; b = b+1 else return a end \
         end) \
         local x = assert(load(d)) \
         return x() == nil",
        true,
    );
    // single-upvalue main-chunk shape still receives globals so global
    // reads through `_ENV` keep working post-load.
    check_int(
        "local x = assert(load('return 1 + #_G')) return x() >= 1 and 1 or 0",
        1,
    );
}

#[test]
fn warn_library_5_4_plus() {
    // PUC 5.4+: warn defaults to off, `@on` enables, `@off` disables,
    // unknown `@<word>` ignored; multi-arg concatenates as one message.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval(
        "warn('silent before @on') \
         warn('@on') \
         warn('hello') \
         warn('multi', '-', 'arg') \
         warn('@unknown')  -- ignored, no emit \
         warn('@off') \
         warn('silent after @off')",
    )
    .expect("warn calls succeed");
    let log = vm.warn_log_take();
    let lines: Vec<String> = log
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .collect();
    assert_eq!(lines, vec!["hello".to_string(), "multi-arg".to_string()]);
}

#[test]
fn warn_library_absent_on_5_3() {
    // PUC 5.3 has no `warn` in the base library. The global resolves to nil
    // and calling it should raise `attempt to call a nil value`.
    let mut vm = Vm::new(LuaVersion::Lua53);
    let result = vm.eval("warn('test')");
    match result {
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("attempt to call") || msg.contains("nil value"),
                "expected nil-call error, got: {msg}"
            );
        }
        Ok(_) => panic!("warn should be absent on 5.3 (no error)"),
    }
}

#[test]
fn os_execute_shell_probe_and_command() {
    // 5.5: no-arg `os.execute()` returns true (shell available); 5.1 returns 1.
    check_bool("return os.execute() == true", true);
    let mut vm51 = Vm::new(LuaVersion::Lua51);
    let v = vm51.eval("return os.execute()").expect("5.1 probe ok");
    assert_eq!(v.len(), 1);
    match v[0] {
        Value::Int(1) => {}
        v => panic!("5.1 os.execute() expected Int(1), got {v:?}"),
    }
    // 5.5: a real shell command. `(true, "exit", 0)` on success;
    // `(nil, "exit", N)` on a non-zero exit (luaL_execresult pushes fail,
    // which is nil). Build the assertion from the triple so we exercise the
    // full return shape.
    check_str(
        "local ok, kind, code = os.execute('true') \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"true:exit:0",
    );
    check_str(
        "local ok, kind, code = os.execute('exit 7') \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"nil:exit:7",
    );
}

#[test]
fn os_exit_is_callable_function() {
    // We can't actually call os.exit (it would tear the test process down),
    // but its absence in 5.1+ would surface as `not a function`. Probe via
    // `type(os.exit)` to confirm registration without invoking it.
    check_str("return type(os.exit)", b"function");
}
