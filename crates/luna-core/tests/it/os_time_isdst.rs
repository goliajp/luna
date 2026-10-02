//! A true `isdst` in `os.time`'s table. PUC hands it to the C library's
//! `mktime`, whose answer for a zone without daylight saving time depends
//! on the library (newer glibc moves the result an hour back, older glibc
//! fails), so the diff_puc fixtures leave it out. luna computes in UTC and
//! follows newer glibc: the result moves an hour back.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn eval_str(version: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(version);
    let r = vm.eval(src).expect("chunk runs");
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn a_true_isdst_moves_the_result_an_hour_back() {
    for v in VERSIONS {
        let got = eval_str(
            v,
            "local t = { year = 2020, month = 1, day = 1, hour = 0 }\n\
             local plain = os.time(t)\n\
             t.isdst = true\n\
             local dst = os.time(t)\n\
             t = { year = 2020, month = 1, day = 1, hour = 0, isdst = 0 }\n\
             return tostring(plain - dst) .. ' ' .. tostring(plain - os.time(t))",
        );
        assert_eq!(got, "3600 3600", "{v:?}");
    }
}

#[test]
fn a_shift_onto_minus_one_follows_each_dialect() {
    for v in VERSIONS {
        let got = eval_str(
            v,
            "local t = { year = 1970, month = 1, day = 1, hour = 1, min = 0, sec = -1, isdst = true }\n\
             local ok, r = pcall(os.time, t)\n\
             return tostring(ok) .. ' ' .. tostring(r)",
        );
        if matches!(v, LuaVersion::Lua51 | LuaVersion::Lua52) {
            assert_eq!(got, "true nil", "{v:?}");
        } else {
            assert!(
                got.starts_with("false ") && got.contains("cannot be represented"),
                "{v:?}: {got}"
            );
        }
    }
}
