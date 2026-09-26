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

/// What `lua.c`'s `collectargs` found in the options.
#[derive(Default)]
struct LuaArgs {
    has_i: bool,
    has_v: bool,
    has_e: bool,
    /// `-E`
    ignore_env: bool,
    /// Index of the script name in `argv`, if there is one.
    script: Option<usize>,
}

/// `lua.c`'s `collectargs` of each dialect. `Err` holds the index of the
/// bad option (5.1 reports none, and takes it only for the usage).
fn collectargs(v: LuaVersion, argv: &[String]) -> Result<LuaArgs, usize> {
    let mut args = LuaArgs::default();
    let mut i = 1;
    while i < argv.len() {
        let a = argv[i].as_bytes();
        if a.first() != Some(&b'-') {
            args.script = Some(i);
            return Ok(args);
        }
        let tail = a.len() > 2;
        match a.get(1) {
            Some(b'-') => {
                if tail {
                    return Err(i);
                }
                args.script = (i + 1 < argv.len()).then_some(i + 1);
                return Ok(args);
            }
            None => {
                args.script = Some(i);
                return Ok(args);
            }
            // 5.2 checks no characters after -E
            Some(b'E') if v == LuaVersion::Lua52 || (v >= LuaVersion::Lua53 && !tail) => {
                args.ignore_env = true;
            }
            Some(b'W') if v >= LuaVersion::Lua54 && !tail => {}
            Some(b'i' | b'v') if !tail => {
                args.has_i |= a[1] == b'i';
                args.has_v = true;
            }
            Some(o @ (b'e' | b'l')) => {
                args.has_e |= *o == b'e';
                if !tail {
                    i += 1;
                    // 5.2 on refuse another option as the argument
                    let missing = match argv.get(i) {
                        None => true,
                        Some(next) => v >= LuaVersion::Lua52 && next.starts_with('-'),
                    };
                    if missing {
                        return Err(i - 1);
                    }
                }
            }
            _ => return Err(i),
        }
        i += 1;
    }
    Ok(args)
}

/// `lua.c`'s `print_usage` of each dialect.
fn print_usage(v: LuaVersion, progname: &str, badoption: &str) {
    let mut out = String::new();
    if v == LuaVersion::Lua51 {
        out.push_str(&format!(
            "usage: {progname} [options] [script [args]].\n\
             Available options are:\n\
             \x20 -e stat  execute string 'stat'\n\
             \x20 -l name  require library 'name'\n\
             \x20 -i       enter interactive mode after executing 'script'\n\
             \x20 -v       show version information\n\
             \x20 --       stop handling options\n\
             \x20 -        execute stdin and stop handling options\n"
        ));
    } else {
        out.push_str(&format!("{progname}: "));
        if matches!(badoption.as_bytes().get(1), Some(b'e' | b'l')) {
            out.push_str(&format!("'{badoption}' needs argument\n"));
        } else {
            out.push_str(&format!("unrecognized option '{badoption}'\n"));
        }
        out.push_str(&format!("usage: {progname} [options] [script [args]]\n"));
        out.push_str("Available options are:\n");
        out.push_str(match v {
            LuaVersion::Lua52 => {
                "  -e stat  execute string 'stat'\n\
                 \x20 -i       enter interactive mode after executing 'script'\n\
                 \x20 -l name  require library 'name'\n\
                 \x20 -v       show version information\n\
                 \x20 -E       ignore environment variables\n\
                 \x20 --       stop handling options\n\
                 \x20 -        stop handling options and execute stdin\n"
            }
            LuaVersion::Lua53 => {
                "  -e stat  execute string 'stat'\n\
                 \x20 -i       enter interactive mode after executing 'script'\n\
                 \x20 -l name  require library 'name' into global 'name'\n\
                 \x20 -v       show version information\n\
                 \x20 -E       ignore environment variables\n\
                 \x20 --       stop handling options\n\
                 \x20 -        stop handling options and execute stdin\n"
            }
            _ => {
                "  -e stat   execute string 'stat'\n\
                 \x20 -i        enter interactive mode after executing 'script'\n\
                 \x20 -l mod    require library 'mod' into global 'mod'\n\
                 \x20 -l g=mod  require library 'mod' into global 'g'\n\
                 \x20 -v        show version information\n\
                 \x20 -E        ignore environment variables\n\
                 \x20 -W        turn warnings on\n\
                 \x20 --        stop handling options\n\
                 \x20 -         stop handling options and execute stdin\n"
            }
        });
    }
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(out.as_bytes()); // nowhere left to report a failed write
}

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

/// The interpreter: the state `lua.c` keeps around its `lua_State`.
struct Interp {
    vm: Vm,
    /// `progname`: `argv[0]`; none while the REPL runs.
    progname: Option<String>,
}

impl Interp {
    fn version(&self) -> LuaVersion {
        self.vm.version()
    }

    /// `lua.c`'s `l_message`.
    fn message(&self, msg: &[u8]) {
        let mut line = Vec::new();
        if let Some(p) = &self.progname {
            line.extend_from_slice(p.as_bytes());
            line.extend_from_slice(b": ");
        }
        // printed with "%s": a C string ends at its first NUL
        let msg = msg.split(|&b| b == 0).next().unwrap_or_default();
        line.extend_from_slice(msg);
        line.push(b'\n');
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(&line); // nowhere left to report a failed write
    }

    /// `lua.c`'s `report` of each dialect, for a chunk that failed with
    /// `err` (the message handler's result, or the load error).
    fn report(&mut self, err: Value) {
        let v = self.version();
        // 5.1 and 5.2 print nothing for a nil error object
        if err.is_nil() && v <= LuaVersion::Lua52 {
            return;
        }
        let msg = match lua_tostring(&mut self.vm, err) {
            Some(m) => m,
            None => match v {
                LuaVersion::Lua51 | LuaVersion::Lua52 => b"(error object is not a string)".to_vec(),
                // 5.3 hands printf a NULL string, which the C libraries luna
                // is compared against (glibc, macOS) print as "(null)"
                LuaVersion::Lua53 => b"(null)".to_vec(),
                _ => b"(error message not a string)".to_vec(),
            },
        };
        self.message(&msg);
    }

    /// `lua.c`'s `docall`: call `f` with `args` under the message handler.
    fn docall(&mut self, f: Value, args: &[Value]) -> Result<Vec<Value>, Value> {
        let msgh = self.vm.native(msghandler);
        self.vm
            .call_value_with_handler(f, args, msgh)
            .map_err(|e| e.0)
    }

    /// `dochunk`: run a loaded chunk, reporting a failure to load or run it.
    /// The values it returned when it ran to the end.
    fn dochunk(&mut self, loaded: Result<Value, LuaError>, args: &[Value]) -> Option<Vec<Value>> {
        let result = match loaded {
            Ok(f) => self.docall(f, args),
            Err(e) => Err(e.0),
        };
        result.map_err(|e| self.report(e)).ok()
    }

    /// `dostring`: a chunk from the command line or the environment. 5.5
    /// takes text only.
    fn dostring(&mut self, src: &[u8], chunkname: &[u8]) -> Option<Vec<Value>> {
        let mode = (self.version() >= LuaVersion::Lua55).then_some(&b"t"[..]);
        let loaded = self.vm.load_buffer(src, chunkname, mode);
        self.dochunk(loaded, &[])
    }

    /// `dofile`: a file, or stdin when `name` is `None`.
    fn dofile(&mut self, name: Option<&[u8]>) -> Option<Vec<Value>> {
        let loaded = self.vm.load_file(name, None);
        self.dochunk(loaded, &[])
    }

    /// `handle_luainit`: run `LUA_INIT` (from 5.2 on `LUA_INIT_5_x` first),
    /// a chunk or, after an `@`, a file to run. False when it failed.
    fn handle_luainit(&mut self) -> bool {
        let versioned = match self.version() {
            LuaVersion::Lua51 => None,
            LuaVersion::Lua52 => Some("LUA_INIT_5_2"),
            LuaVersion::Lua53 => Some("LUA_INIT_5_3"),
            LuaVersion::Lua54 | LuaVersion::MacroLua => Some("LUA_INIT_5_4"),
            LuaVersion::Lua55 => Some("LUA_INIT_5_5"),
        };
        let found = versioned
            .into_iter()
            .chain(["LUA_INIT"])
            .find_map(|name| std::env::var_os(name).map(|init| (name, init)));
        let Some((name, init)) = found else {
            return true;
        };
        let init = os_bytes(init);
        let done = match init.strip_prefix(b"@") {
            Some(file) => self.dofile(Some(file)),
            None => self.dostring(&init, format!("={name}").as_bytes()),
        };
        done.is_some()
    }

    /// `dolibrary`: `-l name`, `require(module)`, whose result 5.2 on store
    /// in a global. From 5.4 on, `g=mod` names the global, and without it a
    /// `-suffix` of the module name is left out of the global's.
    fn dolibrary(&mut self, spec: &str) -> bool {
        let (global, module) = match spec.split_once('=') {
            Some((g, m)) if self.version() >= LuaVersion::Lua54 => (g, m),
            _ if self.version() >= LuaVersion::Lua54 => {
                (spec.split('-').next().unwrap_or_default(), spec)
            }
            _ => (spec, spec),
        };
        let require = self.vm.globals().get(self.str_value("require"));
        let name = self.str_value(module);
        match self.docall(require, &[name]) {
            Ok(_) if self.version() == LuaVersion::Lua51 => true,
            Ok(vals) => {
                let v = vals.first().copied().unwrap_or(Value::Nil);
                // the globals table is not ours to refuse; a failure here
                // would be luna's own
                self.vm.set_global(global, v).expect("set the -l global");
                true
            }
            Err(e) => {
                self.report(e);
                false
            }
        }
    }

    fn str_value(&mut self, s: &str) -> Value {
        Value::Str(self.vm.heap.intern(s.as_bytes()))
    }

    /// `createargtable` / `getargs`: `arg[0]` is the script (`argv[0]` when
    /// there is none), the script's arguments count up from 1 and what
    /// comes before it down from -1.
    fn set_arg(&mut self, argv: &[String], script: usize) {
        let t = self.vm.heap.new_table();
        for (i, a) in argv.iter().enumerate() {
            let k = Value::Int(i as i64 - script as i64);
            let v = self.str_value(a);
            // SAFETY: CLI driver — `t` was just allocated and is reachable
            // only from here until it is stored as `arg`.
            unsafe { t.as_mut() }
                .set(&mut self.vm.heap, k, v)
                .expect("integer keys are valid table keys");
        }
        self.vm
            .set_global("arg", Value::Table(t))
            .expect("set the arg global");
    }

    /// `handle_script`: load the script (stdin for `-` unless it follows
    /// `--`) and run it with its arguments.
    fn handle_script(&mut self, argv: &[String], script: usize) -> bool {
        let v = self.version();
        if v <= LuaVersion::Lua52 {
            self.set_arg(argv, script);
        }
        let stdin = argv[script] == "-" && argv[script - 1] != "--";
        let name = (!stdin).then(|| argv[script].as_str());
        // 5.2 on load either kind of chunk; 5.1 had no mode
        let f = match self.vm.load_file(name.map(str::as_bytes), None) {
            Ok(f) => f,
            Err(e) => {
                self.report(e.0);
                return false;
            }
        };
        let args = if v <= LuaVersion::Lua52 {
            argv[script + 1..]
                .iter()
                .map(|a| self.str_value(a))
                .collect()
        } else {
            match self.pushargs() {
                Ok(args) => args,
                Err(msg) => {
                    self.report(msg);
                    return false;
                }
            }
        };
        self.dochunk(Ok(f), &args).map(show).is_some()
    }

    /// 5.3's `pushargs`: the script's arguments are `arg[1..#arg]`, as they
    /// are after `-e` / `-l` ran.
    fn pushargs(&mut self) -> Result<Vec<Value>, Value> {
        let arg = self.vm.globals().get(self.str_value("arg"));
        let Value::Table(t) = arg else {
            return Err(self.str_value("'arg' is not a table"));
        };
        Ok((1..=t.len()).map(|i| t.get(Value::Int(i))).collect())
    }

    /// `runargs`: the `-e`, `-l` and (5.4 on) `-W` options before `optlim`,
    /// in order; false when one failed.
    fn runargs(&mut self, argv: &[String], optlim: usize) -> bool {
        let mut i = 1;
        while i < optlim {
            let a = &argv[i];
            match a.as_bytes()[1] {
                o @ (b'e' | b'l') => {
                    let extra = if a.len() > 2 {
                        a[2..].to_string()
                    } else {
                        i += 1;
                        argv[i].clone()
                    };
                    let ok = if o == b'e' {
                        self.dostring(extra.as_bytes(), b"=(command line)")
                            .map(show)
                            .is_some()
                    } else {
                        self.dolibrary(&extra)
                    };
                    if !ok {
                        return false;
                    }
                }
                b'W' if self.version() >= LuaVersion::Lua54 => {
                    let warn = self.vm.globals().get(self.str_value("warn"));
                    let on = self.str_value("@on");
                    self.vm
                        .call_value(warn, &[on])
                        .expect("warn(\"@on\") only switches the warning state");
                }
                _ => {}
            }
            i += 1;
        }
        true
    }
}

/// luna's addition to `lua.c`: the values a chunk returned, printed.
fn show(vals: Vec<Value>) {
    for v in vals {
        println!("=> {}", render(v));
    }
}

/// An environment variable's value as the C library hands it over.
fn os_bytes(s: std::ffi::OsString) -> Vec<u8> {
    #[cfg(unix)]
    {
        std::os::unix::ffi::OsStringExt::into_vec(s)
    }
    #[cfg(not(unix))]
    {
        s.to_string_lossy().into_owned().into_bytes()
    }
}

/// `lua_tostring` of an error object: strings, and numbers converted; None
/// for anything else.
fn lua_tostring(vm: &mut Vm, v: Value) -> Option<Vec<u8>> {
    match v {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        Value::Int(_) | Value::Float(_) => Some(vm.error_display(&LuaError(v)).into_bytes()),
        _ => None,
    }
}

/// `lua.c`'s message handler of each dialect: the error message with a
/// traceback of where it was raised (level 1 skips the handler itself).
fn msghandler(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let err = vm.nat_arg(fs, nargs, 0);
    let out = match vm.version() {
        LuaVersion::Lua51 => traceback_51(vm, err)?,
        LuaVersion::Lua52 => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg),
            None if err.is_nil() => err,
            // `luaL_callmeta`: whatever `__tostring` returns
            None => match vm.metafield(err, "__tostring") {
                Value::Nil => Value::Str(vm.heap.intern(b"(no error message)")),
                mm => vm
                    .call_value(mm, &[err])?
                    .first()
                    .copied()
                    .unwrap_or(Value::Nil),
            },
        },
        _ => match lua_tostring(vm, err) {
            Some(msg) => traceback(vm, &msg),
            None => {
                let mm = vm.metafield(err, "__tostring");
                let text = if mm.is_nil() {
                    None
                } else {
                    vm.call_value(mm, &[err])?.first().copied()
                };
                match text {
                    // a string from `__tostring` is the message, as it is
                    Some(s @ Value::Str(_)) => s,
                    _ => {
                        let msg = format!("(error object is a {} value)", err.type_name());
                        traceback(vm, msg.as_bytes())
                    }
                }
            }
        },
    };
    Ok(vm.nat_return(fs, &[out]))
}

/// `luaL_traceback(L, L, msg, 1)` from the message handler.
fn traceback(vm: &mut Vm, msg: &[u8]) -> Value {
    let tb = vm.traceback(Some(msg), 1);
    Value::Str(vm.heap.intern(&tb))
}

/// 5.1 lua.c's `traceback`: a string (or number) message goes through the
/// global `debug.traceback(msg, 2)` when there is one; anything else, or no
/// such function, leaves the message as it is.
fn traceback_51(vm: &mut Vm, err: Value) -> Result<Value, LuaError> {
    if !matches!(err, Value::Str(_) | Value::Int(_) | Value::Float(_)) {
        return Ok(err);
    }
    let key = Value::Str(vm.heap.intern(b"debug"));
    let Value::Table(debug) = vm.globals().get(key) else {
        return Ok(err);
    };
    let key = Value::Str(vm.heap.intern(b"traceback"));
    let tb = debug.get(key);
    if !matches!(tb, Value::Closure(_) | Value::Native(_)) {
        return Ok(err);
    }
    Ok(vm
        .call_value(tb, &[err, Value::Int(2)])?
        .first()
        .copied()
        .unwrap_or(Value::Nil))
}

fn new_vm(opts: &LunaOpts, ignore_env: bool) -> Vm {
    // luna-core's `Vm::new` defaults to the no-op
    // JIT backend; the `luna` bin always wants Cranelift, so go
    // through the wrapper. --no-jit then opts back out.
    let mut vm = if opts.sandbox {
        // SandboxBuilder lives in luna-core and defaults to no JIT
        // already, so --no-jit + --sandbox is automatic. If --sandbox
        // without --no-jit, install Cranelift afterwards (so the
        // builder's safe-stdlib whitelist still applies but the JIT
        // is on).
        let mut vm = luna_jit::vm::Vm::sandbox(opts.version)
            .open_base()
            .open_math()
            .open_string()
            .open_table()
            .open_coroutine()
            .build();
        if !opts.no_jit {
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
