use super::*;

/// `errno` after each call, from `tests/crt_text/errside.c` run with
/// `errno` preset to 99 (99 means the call left it alone).
fn table(lib: Lib) -> Vec<(String, i32)> {
    let text = match lib {
        Lib::Ucrt => include_str!("../../tests/crt_text/errside-ucrt.txt"),
        Lib::Glibc => include_str!("../../tests/crt_text/errside-glibc.txt"),
    };
    text.lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r').trim_end();
            let (name, v) = l.rsplit_once(' ')?;
            Some((name.trim().to_string(), v.parse().ok()?))
        })
        .collect()
}

fn effect(e: Option<i32>) -> i32 {
    e.unwrap_or(99)
}

/// The rule for each measured call, by the name `errside.c` printed.
fn rule(lib: Lib, name: &str) -> Option<i32> {
    let s = |text: &str, exact: bool| {
        let x = crate::numeric::strtod_str(text.as_bytes()).expect("a numeral");
        effect(strtod_errno(lib, x, true, exact))
    };
    let m = |f: MathFn, x: f64, r: f64| effect(math1_errno(lib, f, x, r));
    Some(match name {
        "strtod 1.5" => s("1.5", true),
        "strtod 0x1p3" => s("0x1p3", true),
        "strtod 1e999" => s("1e999", false),
        "strtod -1e999" => s("-1e999", false),
        "strtod 1e-999" => s("1e-999", false),
        "strtod 4e-320" => s("4e-320", false),
        "strtod 2.2250738585072011e-308" => s("2.2250738585072011e-308", false),
        "strtod 0x1p-1074" => s("0x1p-1074", true),
        "strtod 0x1p-1080" => s("0x1p-1080", false),
        "strtod 0x1p2000" => s("0x1p2000", false),
        "log 2" => m(MathFn::Log, 2.0, 2f64.ln()),
        "log 0" => m(MathFn::Log, 0.0, f64::NEG_INFINITY),
        "log -1" => m(MathFn::Log, -1.0, f64::NAN),
        "log10 0" => m(MathFn::Log10, 0.0, f64::NEG_INFINITY),
        "log2 0" => m(MathFn::Log2, 0.0, f64::NEG_INFINITY),
        "exp 1" => m(MathFn::Exp, 0.5, 0.5f64.exp()),
        "exp 1000" => m(MathFn::Exp, 1000.0, f64::INFINITY),
        "exp -1000" => m(MathFn::Exp, -1000.0, 0.0),
        "sqrt 2" => m(MathFn::Sqrt, 2.0, 2f64.sqrt()),
        "sqrt -1" => m(MathFn::Sqrt, -1.0, f64::NAN),
        "acos 2" => m(MathFn::Acos, 2.0, f64::NAN),
        "asin 2" => m(MathFn::Asin, 2.0, f64::NAN),
        "acos 0.5" => m(MathFn::Acos, 0.5, 0.5f64.acos()),
        "sin 1e300" => m(MathFn::Sin, 1e300, 1e300f64.sin()),
        "sin inf" => m(MathFn::Sin, f64::INFINITY, f64::NAN),
        "cos inf" => m(MathFn::Cos, f64::INFINITY, f64::NAN),
        "tan inf" => m(MathFn::Tan, f64::INFINITY, f64::NAN),
        "sinh 1000" => m(MathFn::Sinh, 1000.0, f64::INFINITY),
        "cosh 1000" => m(MathFn::Cosh, 1000.0, f64::INFINITY),
        "pow 2 3" => effect(pow_errno(lib, 2.0, 3.0, 8.0)),
        "pow 10 400" => effect(pow_errno(lib, 10.0, 400.0, f64::INFINITY)),
        "pow 10 -400" => effect(pow_errno(lib, 10.0, -400.0, 0.0)),
        "pow 0 -1" => effect(pow_errno(lib, 0.0, -1.0, f64::INFINITY)),
        "pow -1 0.5" => effect(pow_errno(lib, -1.0, 0.5, f64::NAN)),
        "pow 0 0" => effect(pow_errno(lib, 0.0, 0.0, 1.0)),
        "fmod 5 3" => effect(fmod_errno(5.0, 3.0)),
        "fmod 1 0" => effect(fmod_errno(1.0, 0.0)),
        "fmod inf 1" => effect(fmod_errno(f64::INFINITY, 1.0)),
        "ldexp 1 5000" => effect(ldexp_errno(lib, 1.0, f64::INFINITY)),
        "ldexp 1 -5000" => effect(ldexp_errno(lib, 1.0, 0.0)),
        "ldexp 1 3" => effect(ldexp_errno(lib, 1.0, 8.0)),
        _ => return None,
    })
}

#[test]
fn rules_match_the_measured_libraries() {
    for lib in [Lib::Ucrt, Lib::Glibc] {
        let mut checked = 0;
        for (name, want) in table(lib) {
            if let Some(got) = rule(lib, &name) {
                assert_eq!(got, want, "{lib:?} {name}");
                checked += 1;
            }
        }
        assert_eq!(checked, 41, "{lib:?}: every rule has a measured row");
    }
}

#[test]
fn win32_codes_map_as_the_ucrt_maps_them() {
    use super::errno_of_win32 as m;
    // in the table, at both ends and in the middle
    assert_eq!((m(1), m(2), m(3), m(5), m(13)), (22, 2, 2, 13, 22));
    assert_eq!((m(183), m(206), m(1113), m(1816)), (17, 2, 42, 12));
    // the ranges and the default
    assert_eq!((m(19), m(32), m(36)), (13, 13, 13));
    assert_eq!((m(188), m(202)), (8, 8));
    assert_eq!((m(14), m(123), m(203), m(5000), m(0)), (22, 22, 22, 22, 22));
}
