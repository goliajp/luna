//! The native-panic case shared by luna-core's and luna-jit's tests:
//! `panics.lua` drives a panicking host native through every way Lua code
//! reaches one (calls, tail calls, metamethods, library callbacks,
//! coroutines, hooks, nested protected calls, to-be-closed variables, hot
//! loops); its log, and what the host sees, are pinned per dialect in
//! `panics.txt`.

use std::path::PathBuf;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};

pub const DIALECTS: [(&str, LuaVersion); 5] = [
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

fn boom(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    panic!("boom");
}

fn boom42(_vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    std::panic::panic_any(42_i32);
}

fn boomif(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    if vm.nat_arg(fs, nargs, 0).truthy() {
        panic!("boomif");
    }
    Ok(vm.nat_return(fs, &[Value::Int(1)]))
}

fn boomiter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.1/5.2 may hold the control as either representation of a number
    let c = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(c) => c,
        Value::Float(f) => f as i64,
        _ => return Ok(0),
    };
    if c + 1 == 1500 {
        panic!("boomiter");
    }
    Ok(vm.nat_return(fs, &[Value::Int(c + 1)]))
}

/// The four natives `panics.lua` takes as its arguments.
pub fn natives(vm: &mut Vm) -> [Value; 4] {
    [
        vm.native(boom),
        vm.native(boom42),
        vm.native(boomif),
        vm.native(boomiter),
    ]
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../luna-core/tests/native_panic")
        .join(name)
}

/// The section of `panics.txt` for `dialect`.
pub fn expected(dialect: &str) -> String {
    let text = std::fs::read_to_string(fixture("panics.txt")).expect("read panics.txt");
    let text = text.replace("\r\n", "\n");
    let head = format!("== {dialect}\n");
    let start = text.find(&head).expect("dialect section") + head.len();
    let end = text[start..]
        .find("\n== ")
        .map_or(text.len(), |i| start + i);
    text[start..end].trim_end_matches('\n').to_string()
}

fn text(vm: &Vm, e: &LuaError) -> String {
    vm.error_text(e)
}

/// Run `panics.lua` on `vm` and the host-side cases after it; the log.
pub fn run(vm: &mut Vm) -> String {
    let src = std::fs::read(fixture("panics.lua")).expect("read panics.lua");
    let main = vm.load(&src, b"@panics.lua").expect("load panics.lua");
    let args = natives(vm);
    let mut log = match vm.call_value(Value::Closure(main), &args) {
        Ok(v) => match v.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("panics.lua returned {other:?}"),
        },
        Err(e) => panic!("panics.lua failed: {}", text(vm, &e)),
    };
    let [boom, ..] = args;
    // the host calls the native itself
    let e = vm.call_value(boom, &[]).expect_err("direct call");
    log += &format!("\nhost-direct {}", text(vm, &e));
    log += &format!("\nhost-direct-tb {:?}", vm.take_error_traceback());
    vm.set_global("boom", boom).expect("set boom");
    for (tag, chunk) in [
        ("host-eval", "boom()"),
        (
            "host-eval-nested",
            "local function f() boom() end local function g() f() end g()",
        ),
        ("host-eval-tail", "local function f() return boom() end f()"),
    ] {
        let e = vm.eval(chunk).expect_err(tag);
        log += &format!("\n{tag} {}", text(vm, &e));
        log += &format!("\n{tag}-tb {:?}", vm.take_error_traceback());
    }
    let handler = vm
        .eval("return function(m) return 'handled ' .. m end")
        .expect("handler")[0];
    let f = vm
        .eval("return function(x) local r = boom(x) return r end")
        .expect("f")[0];
    let e = vm
        .call_value_with_handler(f, &[Value::Int(1)], handler)
        .expect_err("handler call");
    log += &format!("\nhost-handler {}", text(vm, &e));
    // the Vm keeps working
    let v = vm.eval("return 1 + 1").expect("after panics");
    log += &format!("\nhost-after {:?}", v.first());
    log
}
