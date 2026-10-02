//! `luna` — small Lua runner CLI on top of the luna library.
//!
//! Usage:
//! ```text
//!   luna [luna options] [options] [script [args]]
//!   luna -h | --help                          print this help
//! ```
//!
//! The options, the `arg` table, and how errors are reported and the
//! exit status set follow the standalone interpreter `lua.c` of the
//! selected dialect (default Lua 5.5): an uncaught error prints
//! `<argv[0]>: <message>` and a traceback on stderr and exits with status
//! 1, and a bad option prints `lua.c`'s usage message. `LUA_INIT`, `-E`
//! and the REPL are `lua.c`'s too. luna's own options (`--lua=`,
//! `--sandbox`, ...) may appear anywhere before the script. Values a
//! script or `-e` chunk returns are printed after it finishes.

use luna_jit::VmExt; // brings install_default_jit / install_null_jit dotted-method form
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::{LuaError, Vm};
use std::io::Write;

#[path = "luna/args.rs"]
mod args;
#[path = "luna/interp.rs"]
mod interp;
#[cfg(feature = "repl-line-editor")]
#[path = "luna/line_editor.rs"]
mod line_editor;
#[path = "luna/repl.rs"]
mod repl;

const HELP: &str = "\
luna — a pure-Rust Lua runner

Usage:
  luna [luna options] [options] [script [args]]
  luna -h | --help      print this help

luna options:
  --lua=X        Select dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5; default 5.5)
  --sandbox      Open only safe stdlib subset (base/math/string/table/coroutine);
                 reject precompiled bytecode loading. Use for untrusted scripts.
  --budget=N     Cap dispatcher to N instructions before raising
                 \"instruction budget exceeded\".
  --no-jit       Install NullJitBackend (interpreter-only).
  --profile      On exit, print compiled-trace counters (trace_compiled_count,
                 trace_dispatched_count, ...) for tuning runs.

Options, as the selected dialect's lua.c takes them:
  -e stat        execute string 'stat'
  -i             enter interactive mode after executing 'script'
  -l mod         require library 'mod' into global 'mod'
  -l g=mod       require library 'mod' into global 'g' (5.4, 5.5)
  -v             show version information
  -E             ignore environment variables (5.2 on)
  -W             turn warnings on (5.4, 5.5)
  --             stop handling options
  -              stop handling options and execute stdin

With no script and no -e / -v, luna reads a program from stdin, or prints
its version and starts the REPL when stdin is a terminal. Arguments go
into the `arg` global as lua.c places them. Before the options' chunks
run, LUA_INIT (from 5.2 on LUA_INIT_5_x first) is run, unless -E is
given: a chunk, or `@file`. An uncaught error is reported as lua.c
reports it (message and traceback on stderr) and the exit status is 1.

The REPL is lua.c's: it prompts with _PROMPT / _PROMPT2 on stdout, reads
more lines while a statement is incomplete, tries a line as an
expression first (5.3 on; `=expr` through 5.4), prints the results with
`print` and an error with its traceback. Ctrl-D ends it.";

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

fn dialect_name(version: LuaVersion) -> &'static str {
    match version {
        LuaVersion::Lua51 => "Lua 5.1",
        LuaVersion::Lua52 => "Lua 5.2",
        LuaVersion::Lua53 => "Lua 5.3",
        LuaVersion::Lua54 => "Lua 5.4",
        LuaVersion::MacroLua => "MacroLua (5.4 + @macros)",
        LuaVersion::Lua55 => "Lua 5.5",
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

/// luna's own options, taken out of the command line before `lua.c`'s
/// options are read.
struct LunaOpts {
    version: LuaVersion,
    sandbox: bool,
    budget: Option<i64>,
    no_jit: bool,
    profile: bool,
}

/// Take luna's own options out of `argv` (`argv[0]` stays): those among the
/// options before the script, the arguments of `-e` / `-l` aside.
/// `-h` / `--help` prints the help and exits.
fn take_luna_opts(argv: Vec<String>) -> (LunaOpts, Vec<String>) {
    let mut opts = LunaOpts {
        version: LuaVersion::Lua55,
        sandbox: false,
        budget: None,
        no_jit: false,
        profile: false,
    };
    let mut rest = Vec::with_capacity(argv.len());
    let mut it = argv.into_iter();
    rest.extend(it.next());
    while let Some(a) = it.next() {
        if a == "-h" || a == "--help" {
            println!("{HELP}");
            std::process::exit(0);
        }
        if let Some(v) = a.strip_prefix("--lua=") {
            opts.version = parse_version(v).unwrap_or_else(|| {
                eprintln!("error: unknown --lua={v} (use 5.1 / 5.2 / 5.3 / 5.4 / 5.5)");
                std::process::exit(2);
            });
            continue;
        }
        if let Some(n) = a.strip_prefix("--budget=") {
            opts.budget = Some(n.parse().unwrap_or_else(|_| {
                eprintln!("error: --budget=N expects an integer");
                std::process::exit(2);
            }));
            continue;
        }
        match a.as_str() {
            "--sandbox" => opts.sandbox = true,
            "--no-jit" => opts.no_jit = true,
            "--profile" => opts.profile = true,
            // the end of the options: the rest is lua.c's
            "--" | "-" => {
                rest.push(a);
                rest.extend(it);
                break;
            }
            _ if !a.starts_with('-') => {
                rest.push(a);
                rest.extend(it);
                break;
            }
            "-e" | "-l" => {
                rest.push(a);
                rest.extend(it.next());
            }
            _ => rest.push(a),
        }
    }
    (opts, rest)
}

use args::{LuaArgs, collectargs, print_usage};
use interp::{Interp, lua_tostring, show};
/// `lua.c`'s `print_version`, with luna's name: 5.1 writes it to stderr,
/// later versions to stdout.
fn print_version(v: LuaVersion) {
    let line = format!("luna {} ({})", env!("CARGO_PKG_VERSION"), dialect_name(v));
    if v == LuaVersion::Lua51 {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

fn new_vm(opts: &LunaOpts, ignore_env: bool) -> Vm {
    // luna-core's `Vm::new` defaults to the no-op
    // JIT backend; the `luna` bin always wants Cranelift, so go
    // through the wrapper. --no-jit then opts back out.
    let mut vm = if opts.sandbox {
        // SandboxBuilder lives in luna-core and has no JIT backend, but
        // its trace flags start on; --no-jit switches them off. Without
        // --no-jit, install Cranelift afterwards (so the builder's
        // safe-stdlib whitelist still applies but the JIT is on).
        let mut vm = luna_jit::vm::Vm::sandbox(opts.version)
            .open_base()
            .open_math()
            .open_string()
            .open_table()
            .open_coroutine()
            .build();
        if opts.no_jit {
            vm.install_null_jit();
        } else {
            vm.install_default_jit();
        }
        vm
    } else {
        let mut vm = if opts.no_jit {
            let mut vm = luna_jit::vm::Vm::new_minimal(opts.version);
            vm.install_null_jit();
            vm
        } else {
            luna_jit::new_minimal_with_jit(opts.version)
        };
        // lua.c sets it before it opens the libraries
        vm.set_ignore_env(ignore_env);
        vm.open_all_libs();
        vm
    };
    if let Some(n) = opts.budget {
        vm.set_instr_budget(Some(n));
    }
    // Test knob, deliberately left out of --help: record traces after N
    // back-edges / calls instead of 64, so that short programs (the
    // differential corpora) run through the trace JIT.
    if let Some(n) = std::env::var("LUNA_JIT_HOT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
    {
        vm.jit.trace_hot_threshold = n;
        vm.jit.call_hot_threshold = n;
    }
    vm
}

fn print_profile(vm: &Vm) {
    // Pull JIT counters from the JitState sidecar (A2).
    eprintln!("---");
    eprintln!("profile (trace JIT counters):");
    eprintln!("  trace_closed_count: {}", vm.jit.counters.closed);
    eprintln!("  trace_compiled_count: {}", vm.jit.counters.compiled);
    eprintln!(
        "  trace_compile_failed_count: {}",
        vm.jit.counters.compile_failed
    );
    eprintln!("  trace_dispatched_count: {}", vm.jit.counters.dispatched);
    eprintln!("  trace_deopt_count: {}", vm.jit.counters.deopt);
    eprintln!(
        "  trace_side_trace_started_count: {}",
        vm.jit.counters.side_trace_started
    );
    eprintln!(
        "  trace_side_trace_compiled_count: {}",
        vm.jit.counters.side_trace_compiled
    );
}

/// `lua.c`'s `pmain`, after the options: true when everything ran.
fn pmain(interp: &mut Interp, argv: &[String], args: &LuaArgs) -> bool {
    let v = interp.version();
    if args.has_v {
        print_version(v);
    }
    // 5.3 on create `arg` before anything runs; 5.1 and 5.2 only for a
    // script. With no script, argv[0] is `arg[0]`.
    if v >= LuaVersion::Lua53 {
        interp.set_arg(argv, args.script.unwrap_or(0));
    }
    // 5.1 ran it before looking at the options
    if v >= LuaVersion::Lua52 && !args.ignore_env && !interp.handle_luainit() {
        return false;
    }
    let optlim = args.script.unwrap_or(argv.len());
    if !interp.runargs(argv, optlim) {
        return false;
    }
    if let Some(script) = args.script
        && !interp.handle_script(argv, script)
    {
        return false;
    }
    if args.has_i {
        return repl::repl(interp);
    }
    if args.script.is_none() && !args.has_e && !args.has_v {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            print_version(v);
            return repl::repl(interp);
        }
        // lua.c ignores how this ends: an error in it is reported, and
        // the exit status stays 0
        if let Some(vals) = interp.dofile(None) {
            show(vals);
        }
    }
    true
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let (opts, argv) = take_luna_opts(argv);
    let progname = match argv.first() {
        Some(p) if !p.is_empty() => p.clone(),
        _ => "lua".to_string(),
    };
    let args = collectargs(opts.version, &argv);
    let ignore_env = args.as_ref().is_ok_and(|a| a.ignore_env);
    let mut interp = Interp {
        vm: new_vm(&opts, ignore_env),
        progname: Some(progname.clone()),
    };
    // 5.1 runs LUA_INIT before it looks at the options
    let ok = (opts.version != LuaVersion::Lua51 || interp.handle_luainit())
        && match args {
            Ok(args) => pmain(&mut interp, &argv, &args),
            Err(bad) => {
                print_usage(opts.version, &progname, &argv[bad]);
                false
            }
        };
    if opts.profile {
        print_profile(&interp.vm);
    }
    // lua.c closes the state before it exits, which finalizes open files
    // and so writes out what they still buffer
    drop(interp);
    std::process::exit(if ok { 0 } else { 1 });
}
