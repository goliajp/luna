//! PUC writes every pointer through the C library's `%p` (`tostring` of a
//! table, function, thread or userdata, `file (%p)`), and C libraries spell
//! pointers differently: glibc writes NULL as `(nil)`, musl as `0`, macOS
//! as `0x0`, MSVC as fixed-width upper-case hex without `0x`. luna follows
//! the C library of the platform it is built for. `string.format("%p")`
//! is the exception for NULL: PUC formats the literal `(null)` itself.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[cfg_attr(
    all(target_os = "windows", target_env = "msvc"),
    link(name = "legacy_stdio_definitions")
)]
unsafe extern "C" {
    fn snprintf(buf: *mut c_char, len: usize, fmt: *const c_char, ...) -> c_int;
}

/// The host C library's `%p` of `p`.
fn host(p: usize) -> String {
    let fmt = CString::new("%p").expect("format has no NUL");
    let mut buf = [0 as c_char; 64];
    // SAFETY: `buf` is 64 bytes and its length is passed; `fmt` is a
    // NUL-terminated string holding one `%p`, matched by the one pointer
    let n = unsafe {
        snprintf(
            buf.as_mut_ptr(),
            buf.len(),
            fmt.as_ptr(),
            p as *const c_void,
        )
    };
    assert!(n >= 0 && (n as usize) < buf.len(), "snprintf returned {n}");
    // SAFETY: snprintf NUL-terminated `buf` within its length
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn eval_str(vm: &mut Vm, src: &str) -> String {
    match vm.eval(src).expect("chunk runs").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn vm_with_pointers(v: LuaVersion) -> Vm {
    let mut vm = Vm::new(v);
    vm.set_global("n", Value::LightUserdata(std::ptr::null()))
        .expect("set n");
    vm.set_global("p", Value::LightUserdata(0x1234 as *const ()))
        .expect("set p");
    vm
}

/// What this platform's C library writes for NULL.
fn expected_null() -> &'static str {
    if cfg!(all(target_os = "linux", target_env = "gnu")) {
        "(nil)"
    } else if cfg!(target_env = "musl") {
        "0"
    } else if cfg!(windows) {
        "0000000000000000"
    } else {
        "0x0"
    }
}

fn expected_0x1234() -> &'static str {
    if cfg!(windows) {
        "0000000000001234"
    } else {
        "0x1234"
    }
}

#[test]
fn null_and_non_null_light_userdata() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = vm_with_pointers(v);
        let got = eval_str(&mut vm, "return tostring(n) .. ' | ' .. tostring(p)");
        let want = format!(
            "userdata: {} | userdata: {}",
            expected_null(),
            expected_0x1234()
        );
        assert_eq!(got, want, "{v:?}");
        assert_eq!(
            got,
            format!("userdata: {} | userdata: {}", host(0), host(0x1234))
        );
    }
}

/// `string.format('%p')` (5.4 on): NULL is the literal `(null)`, padded as
/// a string; other pointers are the C library's text.
#[test]
fn format_p() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let mut vm = vm_with_pointers(v);
        let got = eval_str(
            &mut vm,
            "return string.format('[%p] [%p] [%10p] [%-10p] [%p]', n, p, n, p, 1)",
        );
        let p = expected_0x1234();
        let want = format!("[(null)] [{p}] [    (null)] [{p:<10}] [(null)]");
        assert_eq!(got, want, "{v:?}");
    }
}

/// Tables, functions and threads are written as `string.format('%p')`
/// writes their address. (A file's `file (%p)` names its C `FILE`, not
/// the userdata, in PUC.)
#[test]
fn objects_match_format_p() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    let got = eval_str(
        &mut vm,
        "local t = {} local f = function() end local c = coroutine.create(f) \
         return tostring(tostring(t) == 'table: ' .. string.format('%p', t)) .. ' ' .. \
           tostring(tostring(f) == 'function: ' .. string.format('%p', f)) .. ' ' .. \
           tostring(tostring(c) == 'thread: ' .. string.format('%p', c))",
    );
    assert_eq!(got, "true true true");
}
