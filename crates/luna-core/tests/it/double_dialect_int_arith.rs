//! 5.1/5.2 have only doubles, so arithmetic on the integers the VM keeps
//! (the results of `#`, `select('#')`) must give what the doubles give:
//! -0 from negating or multiplying zero, rounding instead of wrapping, and
//! a table key -0 that stays -0. 5.3+ integers are unaffected.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DOUBLES: [LuaVersion; 2] = [LuaVersion::Lua51, LuaVersion::Lua52];

fn eval_str(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    match vm.eval(src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

const PRELUDE: &str = "local z, one, two = #{}, #{1}, #{1, 2}\n";

fn check(src: &str, want: &str) {
    for v in DOUBLES {
        assert_eq!(
            eval_str(v, &format!("{PRELUDE}{src}")),
            want,
            "{v:?}: {src}"
        );
    }
}

#[test]
fn negating_zero_gives_negative_zero() {
    check("return tostring(-z) .. ' ' .. tostring(1 / -z)", "-0 -inf");
}

#[test]
fn zero_times_a_negative_is_negative_zero() {
    check(
        "return tostring(z * -one) .. ' ' .. tostring(1 / (-one * z)) .. ' ' .. tostring(1 / (z * one))",
        "-0 -inf inf",
    );
}

#[test]
fn negative_zero_flows_into_float_arithmetic() {
    check(
        "return tostring((-z) - z) .. ' ' .. tostring((-z) + (-z))",
        "-0 -0",
    );
}

#[test]
fn overflow_rounds_like_doubles() {
    check(
        "local x = two for i = 1, 70 do x = x * two end
         local y = one for i = 1, 64 do y = y + y end
         local s = -one for i = 1, 64 do s = s - one * -s end
         return tostring(x) .. ' ' .. tostring(y) .. ' ' .. tostring(s)",
        "2.3611832414348e+21 1.844674407371e+19 -1.844674407371e+19",
    );
}

#[test]
fn integers_past_two_to_the_53_round() {
    // 2^53 + 1 is not a double; the sum rounds to 2^53
    check(
        "local p = one for i = 1, 53 do p = p * two end
         return tostring(p + one == p) .. ' ' .. string.format('%.0f', p + one)",
        "true 9007199254740992",
    );
}

#[test]
fn modulo_by_zero_is_nan() {
    check("local m = one % z return tostring(m ~= m)", "true");
}

#[test]
fn a_new_key_negative_zero_stays_negative_zero() {
    check(
        "local t = {} t[-z] = 'a' local k, v = next(t)
         return tostring(k) .. ' ' .. tostring(1 / k) .. ' ' .. v .. ' ' .. t[0] .. t[z]",
        "-0 -inf a aa",
    );
}

#[test]
fn an_existing_key_zero_keeps_its_sign() {
    check(
        "local t = {} t[z] = 'a' t[-z] = 'b' local k, v = next(t)
         return tostring(1 / k) .. ' ' .. v",
        "inf b",
    );
}

#[test]
fn a_negative_zero_key_survives_a_rehash() {
    check(
        "local t = {} t[-z] = 'a' for i = 1, 100 do t['k' .. i] = i end
         for k, v in pairs(t) do if v == 'a' then return tostring(1 / k) end end",
        "-inf",
    );
}

#[test]
fn integer_dialects_keep_integer_zero() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let src = format!(
            "{PRELUDE}local t = {{}} t[-z] = 'a' local k = next(t)
             return math.type(-z) .. ' ' .. math.type(z * -one) .. ' ' .. math.type(k)"
        );
        assert_eq!(eval_str(v, &src), "integer integer integer", "{v:?}");
    }
}
