//! `luna` — small Lua runner CLI on top of the luna library.
//!
//! Usage:
//!   luna [--lua=5.X] <script.lua> [args...]   run a file
//!   luna [--lua=5.X] -e "<code>" [args...]    run inline code
//!   luna [--lua=5.X] -                        read stdin to EOF
//!   luna -h | --help                          print this help
//!
//! Defaults to Lua 5.5 with the full standard library opened (matches
//! `Vm::new`). The script's `arg` table is populated with extra positional
//! args, matching PUC behaviour. If the chunk returns values, they're
//! printed after the script finishes.

use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::Vm;
use std::io::Read;

const HELP: &str = "\
luna — a pure-Rust Lua runner

Usage:
  luna [--lua=5.X] <script.lua> [args...]   run a file
  luna [--lua=5.X] -e \"<code>\" [args...]    run inline code
  luna [--lua=5.X] -                        read stdin to EOF
  luna -h | --help                          print this help

--lua=X selects the dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5; default 5.5).
Extra positional args go into the `arg` global as PUC expects.";

fn parse_version(arg: &str) -> Option<LuaVersion> {
    match arg {
        "5.1" => Some(LuaVersion::Lua51),
        "5.2" => Some(LuaVersion::Lua52),
        "5.3" => Some(LuaVersion::Lua53),
        "5.4" => Some(LuaVersion::Lua54),
        "5.5" => Some(LuaVersion::Lua55),
        _ => None,
    }
}

fn render(v: Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Str(s) => format!("{:?}", String::from_utf8_lossy(s.as_bytes())),
        Value::Table(_) => "<table>".into(),
        Value::Closure(_) | Value::Native(_) => "<function>".into(),
        Value::Coro(_) => "<thread>".into(),
        Value::Userdata(_) => "<userdata>".into(),
        Value::LightUserdata(_) => "<lightuserdata>".into(),
    }
}

enum Source {
    File(String),
    Inline(String),
    Stdin,
}

fn populate_arg(vm: &mut Vm, script_name: Option<&str>, extra: &[String]) {
    let t = vm.heap.new_table();
    // PUC `arg[0]` is the script path; `arg[-1]` is the interpreter binary.
    // For -e / stdin chunks `arg[0]` is absent (PUC convention).
    if let Some(name) = script_name {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        let _ = unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(0), k);
    }
    for (i, s) in extra.iter().enumerate() {
        let v = Value::Str(vm.heap.intern(s.as_bytes()));
        let _ = unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(i as i64 + 1), v);
    }
    vm.set_global("arg", Value::Table(t));
}

fn main() {
    let mut version = LuaVersion::Lua55;
    let mut source: Option<Source> = None;
    let mut extra: Vec<String> = Vec::new();
    let mut consumed_source = false;
    let mut it = std::env::args().skip(1).peekable();
    while let Some(a) = it.next() {
        if consumed_source {
            extra.push(a);
            continue;
        }
        if a == "-h" || a == "--help" {
            println!("{HELP}");
            return;
        }
        if let Some(v) = a.strip_prefix("--lua=") {
            version = parse_version(v).unwrap_or_else(|| {
                eprintln!("error: unknown --lua={v} (use 5.1 / 5.2 / 5.3 / 5.4 / 5.5)");
                std::process::exit(2);
            });
            continue;
        }
        if a == "-e" {
            let code = it.next().unwrap_or_else(|| {
                eprintln!("error: -e expects a code string");
                std::process::exit(2);
            });
            source = Some(Source::Inline(code));
            consumed_source = true;
            continue;
        }
        if a == "-" {
            source = Some(Source::Stdin);
            consumed_source = true;
            continue;
        }
        // First positional non-flag arg = script path; subsequent ones flow
        // into the script's `arg` table.
        source = Some(Source::File(a));
        consumed_source = true;
    }
    let Some(source) = source else {
        eprintln!("{HELP}");
        std::process::exit(2);
    };
    let (src, chunkname, script_for_arg) = match source {
        Source::File(p) => {
            let src = std::fs::read(&p).unwrap_or_else(|e| {
                eprintln!("error: read {p}: {e}");
                std::process::exit(1);
            });
            let cn = format!("@{p}");
            (src, cn, Some(p))
        }
        Source::Inline(code) => (code.into_bytes(), "=(inline)".to_string(), None),
        Source::Stdin => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).unwrap_or_else(|e| {
                eprintln!("error: read stdin: {e}");
                std::process::exit(1);
            });
            (buf, "=stdin".to_string(), None)
        }
    };

    let mut vm = Vm::new(version);
    populate_arg(&mut vm, script_for_arg.as_deref(), &extra);

    let cl = match vm.load(&src, chunkname.as_bytes()) {
        Ok(cl) => cl,
        Err(e) => {
            eprintln!("compile error: {e}");
            std::process::exit(1);
        }
    };
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(vals) => {
            for v in vals {
                println!("=> {}", render(v));
            }
        }
        Err(e) => {
            eprintln!("runtime error: {}", vm.error_text(&e));
            std::process::exit(1);
        }
    }
}
