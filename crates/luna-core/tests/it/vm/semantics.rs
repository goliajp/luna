//! Expressions, locals, assignment, control flow, numeric for, tables and globals.

use super::*;

#[test]
fn arithmetic_semantics() {
    check_int("return 1 + 2", 3);
    check_int("return 7 * 6 - 2", 40);
    check_float("return 7 / 2", 3.5);
    if !cfg!(miri) {
        // miri perturbs powf; exactness is asserted natively only
        check_float("return 2 ^ 10", 1024.0);
    }
    check_int("return 7 // 2", 3);
    check_int("return -7 // 2", -4);
    check_int("return 7 % 3", 1);
    check_int("return -7 % 3", 2);
    check_int("return 7 % -3", -2);
    check_float("return 7.5 % 2", 1.5);
    check_float("return -7.5 % 2", 0.5);
    check_float("return 1 + 0.5", 1.5);
    // integer overflow wraps
    check_int("local a = 9223372036854775807 return a + 1", i64::MIN);
    // unary
    check_int("return -(3)", -3);
    check_float("return -(3.5)", -3.5);
    check_int("local x = 5 return -x", -5);
}

#[test]
fn bitwise_semantics() {
    check_int("return 3 & 5", 1);
    check_int("return 3 | 5", 7);
    check_int("return 3 ~ 5", 6);
    check_int("return ~0", -1);
    check_int("return 1 << 4", 16);
    check_int("return 256 >> 4", 16);
    check_int("return 1 << 64", 0);
    check_int("return 1 << 100", 0);
    check_int("return -1 >> 1", i64::MAX);
    check_int("return 1 << -2", 0);
    check_int("return 16 >> -2", 64);
    // float with integral value converts; fractional errors
    check_int("return 3.0 & 5", 1);
    check_error("return 3.5 & 1", "no integer representation");
    check_error("return {} & 1", "bitwise operation");
}

#[test]
fn division_by_zero() {
    check_error("return 1 // 0", "attempt to divide by zero");
    check_error("return 1 % 0", "'n%0'");
    // float division by zero is inf/nan, not an error
    check_float("return 1 / 0", f64::INFINITY);
    check_float("return 1.0 // 0", f64::INFINITY);
    check_float("return 0 / 0", f64::NAN);
}

#[test]
fn concat_result_as_binop_operand() {
    // A CONCAT result is a temporary at the top of the stack; the enclosing
    // binary op must not let the right operand reuse and clobber its register.
    check_int("return (1 .. 2) * 2", 24); // "12" -> 12 via the string __mul
    check_int("return (1 .. 2) + 1", 13);
    check_int("return (\"7\" .. 3) * 2", 146);
    check_bool("return \"a\" .. \"b\" > \"a\"", true);
    check_bool("return not(2+1 > 3*1) and \"a\"..\"b\" > \"a\"", true);
}

#[test]
fn comparison_semantics() {
    check_bool("return 1 < 2", true);
    check_bool("return 2 < 1", false);
    check_bool("return 1 <= 1", true);
    check_bool("return 2 > 1", true);
    check_bool("return 1 >= 2", false);
    check_bool("return 1 == 1.0", true);
    check_bool("return 1 ~= 1.0", false);
    check_bool("return 'a' < 'b'", true);
    check_bool("return 'abc' <= 'abc'", true);
    check_bool("return 'abc' < 'abd'", true);
    // exact int/float boundary comparisons (2^63 rounds!)
    check_bool("return 9223372036854775807 < 9223372036854775808.0", true);
    check_bool("return 9223372036854775807.0 <= 9223372036854775807", false);
    check_bool("local nan = 0/0 return nan == nan", false);
    check_bool("local nan = 0/0 return nan < nan", false);
    check_bool("return 1 == '1'", false);
    check_error("return 1 < 'x'", "attempt to compare number with string");
    check_error("return {} < {}", "attempt to compare two table values");
}

#[test]
fn logic_and_truthiness() {
    check_int("return 1 and 2", 2);
    assert!(eval1("return nil and 2").is_nil()); // and yields the lhs itself
    check_int("return nil or 5", 5);
    check_int("return false or 5", 5);
    check_int("return 1 or 2", 1);
    check_bool("return not nil", true);
    check_bool("return not 0", false); // 0 is truthy in Lua
    check_int("return (nil and 1) or (false or 7)", 7);
    // rhs not evaluated on short-circuit (would error)
    check_int("local t = nil return false and t.x or 3", 3);
}

#[test]
fn locals_scoping_and_assignment() {
    check_int("local a = 1 local b = a + 1 return a + b", 3);
    check_int("local a = 1 do local a = 100 end return a", 1);
    check_int("local a, b, c = 1, 2 return (c == nil) and (a + b) or 0", 3);
    check_int("local a, b = 1, 2, 3 return a + b", 3);
    check_int("local a = 1 a = a + 10 return a", 11);
    check_int("local a, b = 1, 2 a, b = b, a return a * 10 + b", 21);
    check_int("x = 5 return x + 1", 6);
    check_bool("return rawunset == nil", true); // unknown global reads nil
    check_int("local t = {} t.x = 1 t.x = t.x + 1 return t.x", 2);
}

#[test]
fn control_flow() {
    check_int("if true then return 1 else return 2 end", 1);
    check_int("if false then return 1 else return 2 end", 2);
    check_int(
        "if nil then return 1 elseif 0 then return 2 else return 3 end",
        2,
    );
    check_int(
        "local n = 0 local i = 1 while i <= 10 do n = n + i i = i + 1 end return n",
        55,
    );
    check_int(
        "local n = 0 while true do n = n + 1 if n >= 5 then break end end return n",
        5,
    );
    check_int("local n = 0 repeat n = n + 1 until n >= 3 return n", 3);
    // repeat scope extends over the condition
    check_int(
        "local n = 0 repeat local done = n >= 2 n = n + 1 until done return n",
        3,
    );
}

#[test]
fn numeric_for_semantics() {
    check_int("local s = 0 for i = 1, 10 do s = s + i end return s", 55);
    check_int(
        "local s = 0 for i = 10, 1, -1 do s = s + i end return s",
        55,
    );
    check_int("local s = 0 for i = 1, 10, 2 do s = s + i end return s", 25);
    check_int("local n = 0 for i = 1, 0 do n = n + 1 end return n", 0);
    check_int("local n = 0 for i = 1, 1 do n = n + 1 end return n", 1);
    // float limit with integer start/step keeps integer control variable
    check_int("local s = 0 for i = 1, 2.5 do s = s + i end return s", 3);
    check_bool("for i = 1, 2.5 do return i == 1 end", true);
    // float loop
    check_float(
        "local s = 0.0 for x = 0.5, 2.0, 0.5 do s = s + x end return s",
        5.0,
    );
    // overflow-proof count: full-range loop would hang if implemented naively
    check_int(
        "local n = 0 for i = 9223372036854775805, 9223372036854775807 do n = n + 1 end return n",
        3,
    );
    check_error("for i = 1, 10, 0 do end", "'for' step is zero");
    check_error(
        "for i = {}, 10 do end",
        "bad 'for' initial value (number expected, got table)",
    );
    // break in for
    check_int(
        "local s = 0 for i = 1, 100 do if i > 3 then break end s = s + i end return s",
        6,
    );
}

#[test]
fn tables_and_indexing() {
    check_int("local t = {1, 2, 3} return t[1] + t[2] + t[3]", 6);
    check_int("local t = {x = 10, y = 20} return t.x + t.y", 30);
    check_int("local t = {[2 + 2] = 7} return t[4]", 7);
    check_int("local t = {1, 2; x = 3} return t[1] + t[2] + t.x", 6);
    check_int("local t = {} t[1] = 5 t['k'] = 6 return t[1] + t.k", 11);
    check_int("local t = {{1, 2}, {3, 4}} return t[2][1]", 3);
    check_int("return #'hello'", 5);
    check_int("local t = {1, 2, 3} return #t", 3);
    check_int(
        "local t = {} for i = 1, 100 do t[i] = i * 2 end return t[77]",
        154,
    );
    check_bool("local t = {} return t[1] == nil", true);
    check_bool("local t = {a = 1} return t.b == nil", true);
    check_float("local t = {2.0, [3.0] = 9} return t[1.0] + t[3]", 11.0);
    check_error(
        "local x = nil return x.field",
        "attempt to index a nil value",
    );
    check_error("local x = 5 x.field = 1", "attempt to index a number value");
    check_error("local t = {} t[nil] = 1", "table index is nil");
    check_error("local t = {} t[0/0] = 1", "table index is NaN");
}

#[test]
fn concat_semantics() {
    check_str("return 'a' .. 'b' .. 'c'", b"abc");
    check_str("return 1 .. 2", b"12");
    check_str("return 1.5 .. 'x'", b"1.5x");
    check_str("return 'pi=' .. 3.5 .. '!'", b"pi=3.5!");
    check_error("return {} .. 'x'", "attempt to concatenate a table value");
}

#[test]
fn multiple_returns_fixed() {
    let v = eval("return 1, 2, 3");
    assert_eq!(v.len(), 3);
    assert!(v[0].raw_eq(Value::Int(1)));
    assert!(v[2].raw_eq(Value::Int(3)));
    assert_eq!(eval("return").len(), 0);
    assert_eq!(eval("local a = 1").len(), 0);
}

#[test]
fn globals_via_env() {
    check_int("x = 1 y = 2 return x + y", 3);
    check_int(
        "counter = 0 for i = 1, 5 do counter = counter + 1 end return counter",
        5,
    );
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval("answer = 42").unwrap();
    let v = vm.eval("return answer").unwrap();
    assert!(v[0].raw_eq(Value::Int(42)));
}

#[test]
fn error_positions() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let Err(e) = vm.eval("local x = 1\nlocal y = nil\nreturn y.z") else {
        panic!("expected runtime error")
    };
    let msg = vm.error_text(&e);
    assert!(msg.starts_with("eval:3:"), "position missing: {msg}");
}

#[test]
fn numeric_for_coercion_and_bounds() {
    // nextvar.lua: numeric for coerces string bounds (PUC forprep tonumber).
    check_int(
        "local a = 0; for _ = '10', '1', '-2' do a = a + 1 end; return a",
        5,
    );
    // A float limit beyond the integer range gives an empty decreasing loop.
    check_int(
        "local c = 0; for _ = math.maxinteger, 10e100, -1 do c = c + 1 end; return c",
        0,
    );
    check_int(
        "local c = 0; for _ = math.mininteger, -10e100 do c = c + 1 end; return c",
        0,
    );
}

#[test]
fn multi_assign_snapshots_indexed_lhs() {
    // attrib.lua:505 — PUC manual §3.3.3: in `i, a[i], a, j, a[j], a[i+j] =
    // j, i, i, b, j, i`, every LHS index expression and reference must be
    // captured *before* any assignment. Without the snapshot, `a[i+j]` would
    // re-read `a` after `a = 1` and panic on "index a number".
    check_int(
        "local a, i, j, b = {'a', 'b'}, 1, 2; b = a \
         i, a[i], a, j, a[j], a[i+j] = j, i, i, b, j, i \
         assert(i == 2 and b[1] == 1 and a == 1 and j == b and b[2] == 2 and b[3] == 1) \
         return 1",
        1,
    );
    // Smaller PUC manual example: `i, a[i] = i+1, 20` writes a[old_i].
    check_int(
        "local a, i = {}, 3 \
         i, a[i] = i + 1, 20 \
         assert(i == 4 and a[3] == 20 and a[4] == nil) \
         return 1",
        1,
    );
}

#[test]
fn repeat_until_with_captured_body_local_terminates() {
    // a closure capturing a repeat body's local forces a CLOSE on the loop-back
    // path; the close must sit only on that path, not inline between the
    // condition test and the back-jump (which would skip only the CLOSE on a
    // true condition and loop forever). Regression for closure.lua's
    // repeat/until-with-upvalue block.
    check_int(
        "local a = {} local i = 1 \
         repeat local x = i a[i] = function () return x end i = i + 1 \
         until i > 10 \
         return i",
        11,
    );
    // the condition itself may reference the captured body local
    check_int(
        "local a = {} local i = 1 \
         repeat local x = i a[i] = function () i = x + 1; return x end \
         until i > 10 or a[i]() ~= x \
         return (i == 11 and a[1]() == 1 and a[3]() == 3 and i == 4) and 1 or 0",
        1,
    );
}

#[test]
fn return_stat_caps_at_254_results() {
    // calls.lua :573 — OP_Return encodes `nret + 1` in a byte, so 255
    // explicit returns is a compile error; PUC reports "too many returns".
    check_str(
        "local code = 'return 10' .. string.rep(',10', 254) \
         local f, msg = load(code) \
         return tostring(f) .. '|' .. (msg:find('too many returns') and 'ok' or msg)",
        b"nil|ok",
    );
}

#[test]
fn length_border_on_sparse_power_of_two_keys() {
    // a table whose keys are powers of two (1,2,4,...,2^62) all live in the
    // hash part; the length operator must report a small border quickly, not
    // the huge one (PUC unbound_search's linear fallback), else `for i=1,#t`
    // would loop ~2^62 times.
    check_int(
        "local t = {} for i = 62, 0, -1 do t[2^i] = true end \
         local n = #t return (n == 2 or n == 4) and n or -1",
        2,
    );
    // collectgarbage mode switches report the previous mode
    check_str(
        "collectgarbage('incremental') \
         local a = collectgarbage('generational') \
         local b = collectgarbage('incremental') return a..','..b",
        b"incremental,generational",
    );
    // collectgarbage("param", name [,value]) round-trips pacing parameters:
    // setting returns the previous value, getting returns the current one.
    check_int(
        "local old = collectgarbage('param', 'pause', 100) \
         return collectgarbage('param', 'pause')",
        100,
    );
    check_int(
        "collectgarbage('param', 'stepmul', 250) \
         return collectgarbage('param', 'stepmul', 300)",
        250,
    );
}

#[test]
fn runtime_stack_overflow_is_caught() {
    check_bool(
        "local function f() return 1 + f() end local ok = pcall(f) return ok",
        false,
    );
    check_str(
        "local function f() return 1 + f() end local _, e = pcall(f) return e",
        b"eval:1: stack overflow",
    );
}

#[test]
fn class_pattern_end_to_end() {
    check_int(
        "local Account = {} Account.__index = Account \
         function Account.new(b) return setmetatable({balance = b}, Account) end \
         function Account:deposit(v) self.balance = self.balance + v end \
         function Account:get() return self.balance end \
         local a = Account.new(100) a:deposit(20) a:deposit(3) return a:get()",
        123,
    );
}
