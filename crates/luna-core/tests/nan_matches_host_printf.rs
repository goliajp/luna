//! PUC renders numbers through the host C library's `printf`, and C
//! libraries disagree on how a NaN is spelled (its sign, the `+`/space
//! flags, and on Windows its kind). luna follows the platform it runs on.
//! This test asks the host's own `snprintf` for each case, so it checks
//! the rule on every platform CI runs, not only on the one it was written on.

use std::ffi::{CStr, CString, c_char, c_int};

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

// The UCRT defines the printf family inline in its headers; the linkable
// definitions live in this import library.
#[cfg_attr(
    all(target_os = "windows", target_env = "msvc"),
    link(name = "legacy_stdio_definitions")
)]
unsafe extern "C" {
    fn snprintf(buf: *mut c_char, len: usize, fmt: *const c_char, ...) -> c_int;
}

fn host(fmt: &str, x: f64) -> String {
    let fmt = CString::new(fmt).expect("format has no NUL");
    let mut buf = [0 as c_char; 128];
    // SAFETY: `buf` is 128 bytes and its length is passed; `fmt` is a
    // NUL-terminated string holding one floating conversion, matched by
    // the one `double` argument.
    let n = unsafe { snprintf(buf.as_mut_ptr(), buf.len(), fmt.as_ptr(), x) };
    assert!(
        n >= 0 && (n as usize) < buf.len(),
        "snprintf({fmt:?}) returned {n}"
    );
    // SAFETY: snprintf NUL-terminated `buf` within its length
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn luna(src: &str, x: f64) -> String {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_global("x", Value::Float(x)).expect("set x");
    match vm.eval(src).expect("chunk runs").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// Positive and negative quiet NaNs, with and without a payload, the
/// default NaN x86 produces for 0/0, and a signalling NaN.
const NANS: [u64; 6] = [
    0x7FF8_0000_0000_0000,
    0xFFF8_0000_0000_0000,
    0x7FF8_0000_0000_0123,
    0xFFF8_0000_0000_0123,
    0x7FF0_0000_0000_0001,
    0xFFF0_0000_0000_0001,
];

const FORMATS: [&str; 14] = [
    "%f", "%+f", "% f", "%5.1f", "%-6e", "%010g", "%E", "%G", "%a", "%A", "%+a", "%.0f", "%#g",
    "%12.3e",
];

#[test]
fn string_format_spells_nan_like_the_host_printf() {
    for bits in NANS {
        let x = f64::from_bits(bits);
        for fmt in FORMATS {
            assert_eq!(
                luna(&format!("return string.format('{fmt}', x)"), x),
                host(fmt, x),
                "string.format('{fmt}') of NaN bits {bits:#018x}"
            );
        }
    }
}

#[test]
fn tostring_spells_nan_like_the_host_printf() {
    for bits in NANS {
        let x = f64::from_bits(bits);
        // lua_Number2str is "%.14g" through 5.4
        assert_eq!(
            luna("return tostring(x)", x),
            host("%.14g", x),
            "tostring of NaN bits {bits:#018x}"
        );
    }
}
