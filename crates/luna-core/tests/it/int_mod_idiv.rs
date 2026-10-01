//! Integer `%` and `//` in the interpreter: floor semantics for every sign
//! combination, the `MIN % -1` / `MIN // -1` wrap, and the division by zero
//! errors (PUC `luaV_mod` / `luaV_idiv`).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const XS: [i64; 12] = [i64::MIN, -13, -7, -3, -1, 0, 1, 2, 3, 7, 13, i64::MAX];

// exact in i128, then wrapped to i64 the way Lua integers wrap
fn floor_div(a: i64, b: i64) -> i64 {
    let (a, b) = (a as i128, b as i128);
    let q = a / b;
    (if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }) as i64
}

fn floor_mod(a: i64, b: i64) -> i64 {
    (a as i128 - (b as i128) * (floor_div(a, b) as i128)) as i64
}

fn text(v: Value) -> String {
    match v {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        v => panic!("expected a string, got {v:?}"),
    }
}

#[test]
fn integer_mod_and_idiv_floor_for_every_sign() {
    // oracle sanity: the i128 floor matches the textbook cases
    assert_eq!((floor_div(-7, 2), floor_mod(-7, 2)), (-4, 1));
    assert_eq!((floor_div(7, -2), floor_mod(7, -2)), (-4, -1));
    assert_eq!(
        (floor_div(i64::MIN, -1), floor_mod(i64::MIN, -1)),
        (i64::MIN, 0)
    );
    let mut want = Vec::new();
    for a in XS {
        for b in XS {
            if b != 0 {
                want.push(format!("{},{}", floor_mod(a, b), floor_div(a, b)));
            }
        }
    }
    let want = want.join(" ");
    let src = "local xs = {math.mininteger, -13, -7, -3, -1, 0, 1, 2, 3, 7, 13, math.maxinteger}\n\
               local out = {}\n\
               for _, a in ipairs(xs) do\n\
                 for _, b in ipairs(xs) do\n\
                   if b ~= 0 then out[#out + 1] = (a % b) .. ',' .. (a // b) end\n\
                 end\n\
               end\n\
               return table.concat(out, ' ')";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let got = text(vm.eval(src).expect("run")[0]);
        assert_eq!(got, want, "{v:?}");
    }
}

#[test]
fn integer_division_by_zero_raises() {
    let src = "local function m(a, b) return a % b end\n\
               local function d(a, b) return a // b end\n\
               local ok1, e1 = pcall(m, 5, 0)\n\
               local ok2, e2 = pcall(d, 5, 0)\n\
               return tostring(ok1) .. '|' .. e1 .. '|' .. tostring(ok2) .. '|' .. e2";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let got = text(vm.eval(src).expect("run")[0]);
        assert!(got.starts_with("false|"), "{v:?}: {got}");
        assert!(got.contains("attempt to perform 'n%0'"), "{v:?}: {got}");
        assert!(got.contains("|false|"), "{v:?}: {got}");
        assert!(got.ends_with("attempt to divide by zero"), "{v:?}: {got}");
    }
}

/// The constant-divisor form (`MODK`): -1 and 0 take the side exit ahead of
/// the division.
#[test]
fn integer_mod_by_a_constant() {
    let src = "local out = {}\n\
               for _, a in ipairs({math.mininteger, -13, -1, 0, 1, 13, math.maxinteger}) do\n\
                 out[#out + 1] = (a % -1) .. ',' .. (a % 7) .. ',' .. (a % -7) .. ',' .. (a % 1)\n\
               end\n\
               local err = tostring(select(2, pcall(function(a) return a % 0 end, 3)))\n\
               return table.concat(out, ' ') .. '|' .. err";
    let want: Vec<String> = [i64::MIN, -13, -1, 0, 1, 13, i64::MAX]
        .iter()
        .map(|&a| {
            format!(
                "{},{},{},{}",
                floor_mod(a, -1),
                floor_mod(a, 7),
                floor_mod(a, -7),
                floor_mod(a, 1)
            )
        })
        .collect();
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = Vm::new(v);
        let got = text(vm.eval(src).expect("run")[0]);
        let (head, err) = got.split_once('|').expect("separator");
        assert_eq!(head, want.join(" "), "{v:?}");
        assert!(err.ends_with("attempt to perform 'n%0'"), "{v:?}: {err}");
    }
}
