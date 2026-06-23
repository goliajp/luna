//! `luna` — small Lua runner CLI on top of the luna library.
//!
//! Usage:
//!   luna                                      interactive REPL (C1)
//!   luna [--lua=5.X] <script.lua> [args...]   run a file
//!   luna [--lua=5.X] -e "<code>" [args...]    run inline code
//!   luna [--lua=5.X] -                        read stdin to EOF
//!   luna -h | --help                          print this help
//!
//! Defaults to Lua 5.5 with the full standard library opened (matches
//! `Vm::new`). The script's `arg` table is populated with extra positional
//! args, matching PUC behaviour. If the chunk returns values, they're
//! printed after the script finishes.

use luna::VmExt;  // brings install_default_jit / install_null_jit dotted-method form
use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::Vm;
use std::io::{Read, Write};

const HELP: &str = "\
luna — a pure-Rust Lua runner

Usage:
  luna                                      interactive REPL
  luna [opts] <script.lua> [args...]        run a file
  luna [opts] -e \"<code>\" [args...]         run inline code
  luna [opts] -                             read stdin to EOF
  luna -h | --help                          print this help

Options:
  --lua=X        Select dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5; default 5.5)
  --sandbox      Open only safe stdlib subset (base/math/string/table/coroutine);
                 reject precompiled bytecode loading. Use for untrusted scripts.
  --budget=N     Cap dispatcher to N instructions before raising
                 \"instruction budget exceeded\".
  --no-jit       Install NullJitBackend (interpreter-only).
  --profile      On exit, print compiled-trace counters (trace_compiled_count,
                 trace_dispatched_count, ...) for tuning runs.

Extra positional args go into the `arg` global as PUC expects.

In REPL mode each line is first evaluated as an expression (prefixed
with `return`); on syntax error the line is re-evaluated as a
statement so assignments / function definitions work too.";

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
        // SAFETY: CLI driver — pointer / call set up by the binary entry and matches the expected Vm / luna handle contract.
        let _ = unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(0), k);
    }
    for (i, s) in extra.iter().enumerate() {
        let v = Value::Str(vm.heap.intern(s.as_bytes()));
        // SAFETY: CLI driver — pointer / call set up by the binary entry and matches the expected Vm / luna handle contract.
        let _ = unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(i as i64 + 1), v);
    }
    vm.set_global("arg", Value::Table(t)).expect("CLI arg setup");
}

/// Interactive REPL (C1 — single-line for v1.1; C2/C3 add multi-line,
/// history, syntax highlighting in follow-on commits).
///
/// Each line is first tried as an expression (`return <line>`); on
/// syntax error it's retried as a statement so `x = 1` and
/// `function f() ... end` work too. Ctrl-D / EOF exits cleanly.
fn repl(version: LuaVersion) {
    let mut vm = luna::new_with_jit(version);
    eprintln!(
        "luna {} ({}) — interactive REPL. Ctrl-D to exit.",
        env!("CARGO_PKG_VERSION"),
        match version {
            LuaVersion::Lua51 => "Lua 5.1",
            LuaVersion::Lua52 => "Lua 5.2",
            LuaVersion::Lua53 => "Lua 5.3",
            LuaVersion::Lua54 => "Lua 5.4",
            LuaVersion::Lua55 => "Lua 5.5",
        }
    );
    let stdin = std::io::stdin();
    let mut buf = String::new();
    loop {
        eprint!("> ");
        let _ = std::io::stderr().flush();
        buf.clear();
        match stdin.read_line(&mut buf) {
            Ok(0) => {
                eprintln!();
                return;
            }
            Ok(_) => {
                let line = buf.trim();
                if line.is_empty() {
                    continue;
                }
                // Expression-first: `return <line>` to surface a
                // returned value. On syntax error (likely an
                // assignment / def), retry as a statement.
                let as_expr = format!("return {line}");
                let result = match vm.eval(&as_expr) {
                    Ok(vs) => Ok(vs),
                    Err(_) => vm.eval(line),
                };
                match result {
                    Ok(vs) => {
                        for v in vs {
                            println!("{}", render(v));
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {}", vm.error_text(&e));
                    }
                }
            }
            Err(e) => {
                eprintln!("io error: {e}");
                return;
            }
        }
    }
}

fn main() {
    let mut version = LuaVersion::Lua55;
    let mut source: Option<Source> = None;
    let mut extra: Vec<String> = Vec::new();
    let mut consumed_source = false;
    let mut sandbox = false;
    let mut budget: Option<i64> = None;
    let mut no_jit = false;
    let mut profile = false;
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
        if a == "--sandbox" {
            sandbox = true;
            continue;
        }
        if let Some(n) = a.strip_prefix("--budget=") {
            budget = Some(n.parse().unwrap_or_else(|_| {
                eprintln!("error: --budget=N expects an integer");
                std::process::exit(2);
            }));
            continue;
        }
        if a == "--no-jit" {
            no_jit = true;
            continue;
        }
        if a == "--profile" {
            profile = true;
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
        // C1 — no source means interactive REPL.
        repl(version);
        return;
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

    // v1.1 A1 Session C — luna-core's `Vm::new` defaults to the no-op
    // JIT backend; the `luna` bin always wants Cranelift, so go
    // through the wrapper. --no-jit then opts back out.
    let mut vm = if sandbox {
        // SandboxBuilder lives in luna-core and defaults to no JIT
        // already, so --no-jit + --sandbox is automatic. If --sandbox
        // without --no-jit, install Cranelift afterwards (so the
        // builder's safe-stdlib whitelist still applies but the JIT
        // is on).
        let mut vm = luna::vm::Vm::sandbox(version)
            .open_base()
            .open_math()
            .open_string()
            .open_table()
            .open_coroutine()
            .build();
        if !no_jit {
            vm.install_default_jit();
        }
        vm
    } else if no_jit {
        // Full stdlib but no JIT.
        let mut vm = luna::vm::Vm::new(version);
        vm.install_null_jit();
        vm
    } else {
        luna::new_with_jit(version)
    };

    if let Some(n) = budget {
        vm.set_instr_budget(Some(n));
    }

    populate_arg(&mut vm, script_for_arg.as_deref(), &extra);

    // C5 — pretty error rendering: ANSI color when stderr is a TTY
    // and NO_COLOR isn't set. Embedders piping luna output to logs
    // get plain text automatically.
    let color =
        std::env::var_os("NO_COLOR").is_none() && std::io::IsTerminal::is_terminal(&std::io::stderr());

    let cl = match vm.load(&src, chunkname.as_bytes()) {
        Ok(cl) => cl,
        Err(e) => {
            print_pretty_error(&mut vm, &format!("{e}"), &src, color, /*compile=*/ true);
            std::process::exit(1);
        }
    };
    let result = vm.call_value(Value::Closure(cl), &[]);

    if profile {
        // Pull JIT counters from the JitState sidecar (A2).
        eprintln!("---");
        eprintln!("profile (trace JIT counters):");
        eprintln!("  trace_closed_count: {}", vm.jit.counters.closed);
        eprintln!("  trace_compiled_count: {}", vm.jit.counters.compiled);
        eprintln!("  trace_compile_failed_count: {}", vm.jit.counters.compile_failed);
        eprintln!("  trace_dispatched_count: {}", vm.jit.counters.dispatched);
        eprintln!("  trace_deopt_count: {}", vm.jit.counters.deopt);
        eprintln!("  trace_side_trace_started_count: {}", vm.jit.counters.side_trace_started);
        eprintln!("  trace_side_trace_compiled_count: {}", vm.jit.counters.side_trace_compiled);
    }

    match result {
        Ok(vals) => {
            for v in vals {
                println!("=> {}", render(v));
            }
        }
        Err(e) => {
            let msg = vm.error_text(&e);
            print_pretty_error(&mut vm, &msg, &src, color, /*compile=*/ false);
            std::process::exit(1);
        }
    }
}

/// C5 — pretty error rendering with source name / line / context
/// snippet / color. Uses `Vm::error_source` (B6) for the (chunk_name,
/// line) pair and `Vm::take_error_traceback` for the Lua-side
/// traceback. The `src` arg lets us print the offending source line
/// directly when the line is known.
fn print_pretty_error(vm: &mut Vm, msg: &str, src: &[u8], color: bool, compile_time: bool) {
    let (red, dim, bold, reset) = if color {
        ("\x1b[31m", "\x1b[2m", "\x1b[1m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };

    let kind_label = if compile_time {
        "compile error"
    } else {
        "runtime error"
    };
    // For compile errors the bin caller used vm.load() directly so
    // the Vm-side error_kind metadata wasn't populated (eval_chunk
    // sets it but load() is one layer below). Synthesize "syntax"
    // for the compile path; otherwise read from Vm.
    let kind_str = if compile_time {
        "syntax".to_string()
    } else {
        format!("{}", vm.error_kind())
    };
    eprintln!("{bold}{red}{kind_label}{reset} {dim}[{kind_str}]{reset}: {msg}");

    if let Some((chunk, line)) = vm.error_source() {
        eprintln!("  {dim}at {chunk}:{line}{reset}");
        if let Some(snippet) = nth_line(src, line) {
            eprintln!("  {dim}|{reset} {snippet}");
        }
    }
    if let Some(tb) = vm.take_error_traceback() {
        eprintln!("{dim}traceback:{reset}");
        for line in tb.lines() {
            eprintln!("  {line}");
        }
    }
}

/// Pull the `n`th 1-based line from a byte buffer (UTF-8 lossy
/// rendering for the slice). Returns `None` for line 0 or beyond EOF.
fn nth_line(src: &[u8], n: u32) -> Option<String> {
    if n == 0 {
        return None;
    }
    let s = String::from_utf8_lossy(src);
    s.lines().nth((n - 1) as usize).map(|l| l.to_string())
}
