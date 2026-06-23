//! Single-file official-suite runner with a hard timeout, for diagnosing a
//! specific test file. Usage: `runone [--lua=5.X] <file.lua>` (run from repo
//! root). The version flag selects both the `LuaVersion` and the suite dir;
//! it defaults to 5.5 so existing invocations are unchanged.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

fn parse_version(arg: &str) -> Option<(LuaVersion, &'static str)> {
    match arg {
        "5.1" => Some((LuaVersion::Lua51, "tests/official/lua5.1-tests")),
        "5.2" => Some((LuaVersion::Lua52, "tests/official/lua-5.2.2-tests")),
        "5.3" => Some((LuaVersion::Lua53, "tests/official/lua-5.3.4-tests")),
        "5.4" => Some((LuaVersion::Lua54, "tests/official/lua-5.4.8-tests")),
        "5.5" => Some((LuaVersion::Lua55, "tests/official/lua-5.5.0-tests")),
        _ => None,
    }
}

fn main() {
    let args = std::env::args().skip(1);
    let mut version = LuaVersion::Lua55;
    let mut dir = "tests/official/lua-5.5.0-tests";
    let mut path: Option<String> = None;
    for a in args {
        if let Some(v) = a.strip_prefix("--lua=") {
            let (vv, dd) = parse_version(v)
                .unwrap_or_else(|| panic!("unknown --lua={v} (use 5.1/5.2/5.3/5.4/5.5)"));
            version = vv;
            dir = dd;
        } else {
            path = Some(a);
        }
    }
    let path = path.expect("usage: runone [--lua=5.X] <file.lua>");
    std::env::set_current_dir(dir).unwrap();
    let src = std::fs::read(&path).unwrap();
    let label = path.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = luna_jit::new_with_jit(version);
            vm.set_global("_U", Value::Bool(true)).unwrap();
            // Mirror official_run.rs so diagnosis matches the test gate.
            if label == "attrib.lua" || label == "files.lua" {
                vm.set_global("_port", Value::Bool(true)).unwrap();
            }
            if label == "files.lua" {
                vm.set_global("_noposix", Value::Bool(true)).unwrap();
            }
            let chunkname = format!("@{label}");
            let r = match vm.load(&src, chunkname.as_bytes()) {
                Ok(cl) => match vm.call_value(Value::Closure(cl), &[]) {
                    Ok(_) => "PASS".to_string(),
                    Err(e) => format!("FAIL {}", vm.error_text(&e)),
                },
                Err(e) => format!("COMPILE {e}"),
            };
            let _ = tx.send(r);
        })
        .unwrap();
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(r) => eprintln!("=> {r}"),
        Err(_) => eprintln!("=> HANG"),
    }
}
