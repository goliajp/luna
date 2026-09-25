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
//! 1, and a bad option prints `lua.c`'s usage message. luna's own options
//! (`--lua=`, `--sandbox`, ...) may appear anywhere before the script.
//! Values a chunk returns are printed after it finishes.

use luna_jit::VmExt; // brings install_default_jit / install_null_jit dotted-method form
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::{LuaError, Vm};
use std::io::Write;

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

With no script and no -e / -v, luna reads a program from stdin, or starts
the interactive REPL when stdin is a terminal. Arguments go into the `arg`
global as lua.c places them. An uncaught error is reported as lua.c
reports it (message and traceback on stderr) and the exit status is 1.

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

/// Maximum entries persisted in `~/.luna_history`. Older entries get
/// truncated on save. PUC `lua`'s readline-driven history typically
/// keeps 500-1000; pick the higher end since each line is short.
const HISTORY_MAX_ENTRIES: usize = 1000;

fn history_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join(".luna_history"))
}

fn load_history() -> Vec<String> {
    let Some(p) = history_path() else {
        return Vec::new();
    };
    match std::fs::read_to_string(&p) {
        Ok(s) => s.lines().map(|l| l.to_string()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => Vec::new(),
    }
}

fn save_history(entries: &[String]) {
    let Some(p) = history_path() else {
        return;
    };
    let body = entries
        .iter()
        .rev()
        .take(HISTORY_MAX_ENTRIES)
        .rev()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(&p, body);
}

/// True if `msg` indicates the parser ran out of input mid-block
/// (incomplete `if … then`, `do … end`, `function … end`, long string,
/// etc.). Per R-A1 audit: luna's `SyntaxError::msg` carries `near
/// <eof>` / `unfinished … near <eof>` markers exactly when more input
/// would let the parser continue. The single counter-example is the
/// explicit `'<eof>' expected` form, which means the parser saw EXTRA
/// trailing input and is not asking for more.
fn is_incomplete_syntax(msg: &str) -> bool {
    if msg.contains("'<eof>' expected") {
        return false;
    }
    msg.contains(" near <eof>")
}

/// Interactive REPL entry point. v1.3 R3: dispatches to the
/// rustyline-backed editor when built with `--features
/// repl-line-editor` (tab completion against `Vm` globals + Lua
/// syntax highlighting); otherwise falls through to the v1.2 plain
/// path. The default `cargo install luna-jit` keeps a tiny dep
/// surface (no rustyline) by leaving the feature off.
fn repl(vm: &mut Vm) {
    #[cfg(feature = "repl-line-editor")]
    repl_rustyline(vm);
    #[cfg(not(feature = "repl-line-editor"))]
    repl_plain(vm);
}

/// v1.2 plain-stdin REPL — single-line + multi-line continuation +
/// `~/.luna_history`. Always available; the rustyline build falls
/// back here when terminal init fails.
///
/// Each entered chunk is first tried as an expression (`return <chunk>`)
/// to surface a value; on syntax error it's retried as a statement so
/// `x = 1` and `function f() ... end` work too. Mid-block incomplete
/// input (detected via `SyntaxError::msg.contains(" near <eof>")`)
/// reprompts with `>>` instead of erroring. Ctrl-D / EOF exits cleanly
/// and persists the history.
fn repl_plain(vm: &mut Vm) {
    eprintln!(
        "luna {} ({}) — interactive REPL. Ctrl-D to exit.",
        env!("CARGO_PKG_VERSION"),
        dialect_name(vm.version())
    );
    let stdin = std::io::stdin();
    let mut history: Vec<String> = load_history();
    let mut chunk = String::new();
    let mut in_continuation = false;
    loop {
        eprint!("{}", if in_continuation { ">> " } else { "> " });
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => {
                if in_continuation {
                    // Mid-block Ctrl-D — drop the partial chunk and exit.
                    eprintln!();
                }
                eprintln!();
                save_history(&history);
                return;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("io error: {e}");
                save_history(&history);
                return;
            }
        }
        if !in_continuation && line.trim().is_empty() {
            continue;
        }
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(&line);
        // Expression-first: `return <chunk>` to surface a returned
        // value. If the expression parses but the statement form
        // doesn't (e.g. assignments), the statement-form error is
        // what we report to the user.
        let as_expr = format!("return {chunk}");
        let result = match vm.eval(&as_expr) {
            Ok(vs) => Ok(vs),
            Err(_) => vm.eval(chunk.as_str()),
        };
        match result {
            Ok(vs) => {
                for v in vs {
                    println!("{}", render(v));
                }
                history.push(chunk.trim_end().to_string());
                chunk.clear();
                in_continuation = false;
            }
            Err(e) => {
                let msg = vm.error_text(&e);
                if is_incomplete_syntax(&msg) {
                    // More input needed — keep `chunk` and reprompt
                    // with `>>`.
                    in_continuation = true;
                } else {
                    eprintln!("error: {}", msg);
                    history.push(chunk.trim_end().to_string());
                    chunk.clear();
                    in_continuation = false;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// v1.3 R3 — rustyline-backed REPL (opt-in via `--features
// repl-line-editor`).
//
// The non-feature build keeps the v1.2 `repl_plain` path above
// unchanged so the default `cargo install luna-jit` doesn't pick up
// rustyline. luna-core remains 0-dep regardless.
//
// Layered on top of the same eval / multi-line continuation logic:
//   * Tab completion — walks Vm globals (`vm.globals().next(...)`)
//     and offers names whose prefix matches the word at the cursor.
//   * Syntax highlight — small Lua tokenizer (keywords / strings /
//     numbers / line comments / long comments / long strings)
//     emitting ANSI colour escapes via rustyline's `Highlighter`
//     trait. No dep on syntect / tree-sitter.
//   * History — rustyline manages `~/.luna_history` natively;
//     same file the v1.2 path writes, so flipping the feature bit
//     doesn't lose history.
// ─────────────────────────────────────────────────────────────────

#[cfg(feature = "repl-line-editor")]
#[derive(Default)]
struct GlobalsSnapshot {
    names: Vec<String>,
}

#[cfg(feature = "repl-line-editor")]
struct LuaHelper {
    globals: std::rc::Rc<std::cell::RefCell<GlobalsSnapshot>>,
}

#[cfg(feature = "repl-line-editor")]
impl rustyline::completion::Completer for LuaHelper {
    type Candidate = rustyline::completion::Pair;
    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<rustyline::completion::Pair>)> {
        // Lua identifiers: [A-Za-z_][A-Za-z0-9_]*. Dotted-name
        // completion (`string.up<TAB>`) is a follow-up; first-segment
        // matching covers the common case.
        let bytes = line.as_bytes();
        let mut start = pos;
        while start > 0 {
            let c = bytes[start - 1];
            if !(c.is_ascii_alphanumeric() || c == b'_') {
                break;
            }
            start -= 1;
        }
        let prefix = &line[start..pos];
        if prefix.is_empty() {
            return Ok((pos, Vec::new()));
        }
        let snap = self.globals.borrow();
        let mut matches: Vec<rustyline::completion::Pair> = snap
            .names
            .iter()
            .filter(|n| n.starts_with(prefix))
            .map(|n| rustyline::completion::Pair {
                display: n.clone(),
                replacement: n.clone(),
            })
            .collect();
        matches.sort_by(|a, b| a.display.cmp(&b.display));
        matches.dedup_by(|a, b| a.display == b.display);
        Ok((start, matches))
    }
}

#[cfg(feature = "repl-line-editor")]
impl rustyline::hint::Hinter for LuaHelper {
    type Hint = String;
}

#[cfg(feature = "repl-line-editor")]
impl rustyline::validate::Validator for LuaHelper {}

#[cfg(feature = "repl-line-editor")]
impl rustyline::Helper for LuaHelper {}

#[cfg(feature = "repl-line-editor")]
impl rustyline::highlight::Highlighter for LuaHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Owned(highlight_lua(line))
    }
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> std::borrow::Cow<'b, str> {
        std::borrow::Cow::Owned(format!("\x1b[2m{prompt}\x1b[0m"))
    }
    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        // Re-render every keystroke — tokenizer is cheap and partial
        // highlights look broken mid-string / mid-comment.
        true
    }
}

#[cfg(feature = "repl-line-editor")]
fn repl_rustyline(vm: &mut Vm) {
    use rustyline::Editor;
    use rustyline::error::ReadlineError;
    use rustyline::history::DefaultHistory;
    use std::cell::RefCell;
    use std::rc::Rc;

    eprintln!(
        "luna {} ({}) — interactive REPL (rustyline). Ctrl-D to exit.",
        env!("CARGO_PKG_VERSION"),
        dialect_name(vm.version())
    );

    let globals = Rc::new(RefCell::new(GlobalsSnapshot::default()));
    let helper = LuaHelper {
        globals: globals.clone(),
    };

    let mut rl: Editor<LuaHelper, DefaultHistory> = match Editor::new() {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("rustyline init failed ({e}); falling back to plain REPL");
            repl_plain(vm);
            return;
        }
    };
    rl.set_helper(Some(helper));
    let hist_path = history_path();
    if let Some(ref p) = hist_path {
        let _ = rl.load_history(p);
    }

    let mut chunk = String::new();
    let mut in_continuation = false;
    loop {
        refresh_globals_snapshot(vm, &globals);
        let prompt = if in_continuation { ">> " } else { "> " };
        let line = match rl.readline(prompt) {
            Ok(l) => l,
            Err(ReadlineError::Eof) => {
                if let Some(ref p) = hist_path {
                    let _ = rl.save_history(p);
                }
                return;
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C drops the in-flight chunk, mirrors PUC.
                chunk.clear();
                in_continuation = false;
                continue;
            }
            Err(e) => {
                eprintln!("io error: {e}");
                if let Some(ref p) = hist_path {
                    let _ = rl.save_history(p);
                }
                return;
            }
        };
        if !in_continuation && line.trim().is_empty() {
            continue;
        }
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(&line);

        let as_expr = format!("return {chunk}");
        let result = match vm.eval(&as_expr) {
            Ok(vs) => Ok(vs),
            Err(_) => vm.eval(chunk.as_str()),
        };
        match result {
            Ok(vs) => {
                for v in vs {
                    println!("{}", render(v));
                }
                let _ = rl.add_history_entry(chunk.trim_end());
                chunk.clear();
                in_continuation = false;
            }
            Err(e) => {
                let msg = vm.error_text(&e);
                if is_incomplete_syntax(&msg) {
                    in_continuation = true;
                } else {
                    eprintln!("error: {msg}");
                    let _ = rl.add_history_entry(chunk.trim_end());
                    chunk.clear();
                    in_continuation = false;
                }
            }
        }
    }
}

#[cfg(feature = "repl-line-editor")]
fn refresh_globals_snapshot(vm: &mut Vm, snap: &std::rc::Rc<std::cell::RefCell<GlobalsSnapshot>>) {
    // Iterate `_G` via Table::next (the same primitive that backs
    // `pairs`). Non-string keys (rare for globals) are skipped — we
    // only suggest identifier-shaped names. Gc<T>: Deref<Target=T>
    // (heap.rs:154); read-only iteration needs no unsafe block.
    let g = vm.globals();
    let mut key: Value = Value::Nil;
    let mut out: Vec<String> = Vec::new();
    loop {
        match g.next(key) {
            Ok(Some((k, _v))) => {
                if let Value::Str(s) = k {
                    let bytes = s.as_bytes();
                    if !bytes.is_empty()
                        && bytes
                            .iter()
                            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
                        && !bytes[0].is_ascii_digit()
                    {
                        out.push(String::from_utf8_lossy(bytes).into_owned());
                    }
                }
                key = k;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    snap.borrow_mut().names = out;
}

/// Tiny Lua tokenizer → ANSI-coloured string. Used by `LuaHelper`'s
/// `Highlighter` impl. Recognises keywords, short / long strings,
/// short / long comments, decimal + hex number literals; everything
/// else passes through unstyled. Idempotent over the input bytes.
#[cfg(feature = "repl-line-editor")]
fn highlight_lua(src: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    const KW: &str = "\x1b[34m"; // blue
    const STR: &str = "\x1b[33m"; // yellow
    const NUM: &str = "\x1b[35m"; // magenta
    const CMT: &str = "\x1b[2;37m"; // dim white
    const RST: &str = "\x1b[0m";

    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 16);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        // Comment: `-- …` or `--[==[ … ]==]`.
        if c == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            let start = i;
            i += 2;
            if i < bytes.len() && bytes[i] == b'[' {
                let mut k = i + 1;
                let mut level = 0;
                while k < bytes.len() && bytes[k] == b'=' {
                    level += 1;
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'[' {
                    let mut end = k + 1;
                    while end < bytes.len() {
                        if bytes[end] == b']' {
                            let mut m = end + 1;
                            let mut eq = 0;
                            while m < bytes.len() && bytes[m] == b'=' {
                                eq += 1;
                                m += 1;
                            }
                            if eq == level && m < bytes.len() && bytes[m] == b']' {
                                end = m + 1;
                                break;
                            }
                        }
                        end += 1;
                    }
                    let end = end.min(bytes.len());
                    out.push_str(CMT);
                    out.push_str(&src[start..end]);
                    out.push_str(RST);
                    i = end;
                    continue;
                }
            }
            let mut end = i;
            while end < bytes.len() && bytes[end] != b'\n' {
                end += 1;
            }
            out.push_str(CMT);
            out.push_str(&src[start..end]);
            out.push_str(RST);
            i = end;
            continue;
        }
        // Short string literal.
        if c == b'"' || c == b'\'' {
            let quote = c;
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                if bytes[i] == b'\n' {
                    break;
                }
                i += 1;
            }
            let end = i.min(bytes.len());
            out.push_str(STR);
            out.push_str(&src[start..end]);
            out.push_str(RST);
            continue;
        }
        // Long string `[==[ … ]==]`.
        if c == b'[' {
            let mut k = i + 1;
            let mut level = 0;
            while k < bytes.len() && bytes[k] == b'=' {
                level += 1;
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b'[' {
                let start = i;
                let mut end = k + 1;
                while end < bytes.len() {
                    if bytes[end] == b']' {
                        let mut m = end + 1;
                        let mut eq = 0;
                        while m < bytes.len() && bytes[m] == b'=' {
                            eq += 1;
                            m += 1;
                        }
                        if eq == level && m < bytes.len() && bytes[m] == b']' {
                            end = m + 1;
                            break;
                        }
                    }
                    end += 1;
                }
                let end = end.min(bytes.len());
                out.push_str(STR);
                out.push_str(&src[start..end]);
                out.push_str(RST);
                i = end;
                continue;
            }
        }
        // Number literal (decimal / hex / float / exponent).
        if c.is_ascii_digit() || (c == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            let hex =
                c == b'0' && i + 1 < bytes.len() && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X');
            if hex {
                i += 2;
                let mut prev_exp = false;
                while i < bytes.len() {
                    let b = bytes[i];
                    let is_sign_after_p = (b == b'+' || b == b'-') && prev_exp;
                    if b.is_ascii_hexdigit()
                        || b == b'.'
                        || matches!(b, b'p' | b'P')
                        || is_sign_after_p
                    {
                        prev_exp = matches!(b, b'p' | b'P');
                        i += 1;
                    } else {
                        break;
                    }
                }
            } else {
                let mut prev_exp = false;
                while i < bytes.len() {
                    let b = bytes[i];
                    let is_sign_after_e = (b == b'+' || b == b'-') && prev_exp;
                    if b.is_ascii_digit()
                        || b == b'.'
                        || matches!(b, b'e' | b'E')
                        || is_sign_after_e
                    {
                        prev_exp = matches!(b, b'e' | b'E');
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            out.push_str(NUM);
            out.push_str(&src[start..i]);
            out.push_str(RST);
            continue;
        }
        // Identifier or keyword.
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            if KEYWORDS.contains(&word) {
                out.push_str(KW);
                out.push_str(word);
                out.push_str(RST);
            } else {
                out.push_str(word);
            }
            continue;
        }
        // Punctuation / whitespace.
        out.push(c as char);
        i += 1;
    }
    out
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
            Some(b'E') if v == LuaVersion::Lua52 => {}
            Some(b'E') if v >= LuaVersion::Lua53 && !tail => {}
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
    /// True when it ran to the end.
    fn dochunk(&mut self, loaded: Result<Value, LuaError>, args: &[Value]) -> bool {
        let result = match loaded {
            Ok(f) => self.docall(f, args),
            Err(e) => Err(e.0),
        };
        match result {
            Ok(vals) => {
                for v in vals {
                    println!("=> {}", render(v));
                }
                true
            }
            Err(e) => {
                self.report(e);
                false
            }
        }
    }

    /// `dostring`: `-e`'s chunk, named `(command line)`. 5.5 takes text
    /// only.
    fn dostring(&mut self, src: &str) -> bool {
        let mode = (self.version() >= LuaVersion::Lua55).then_some(&b"t"[..]);
        let loaded = self
            .vm
            .load_buffer(src.as_bytes(), b"=(command line)", mode);
        self.dochunk(loaded, &[])
    }

    /// `dofile`: a file, or stdin when `name` is `None`.
    fn dofile(&mut self, name: Option<&str>) -> bool {
        let loaded = self.vm.load_file(name.map(str::as_bytes), None);
        self.dochunk(loaded, &[])
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
        self.dochunk(Ok(f), &args)
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
                        self.dostring(&extra)
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

fn new_vm(opts: &LunaOpts) -> Vm {
    // v1.1 A1 Session C — luna-core's `Vm::new` defaults to the no-op
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
    } else if opts.no_jit {
        // Full stdlib but no JIT.
        let mut vm = luna_jit::vm::Vm::new(opts.version);
        vm.install_null_jit();
        vm
    } else {
        luna_jit::new_with_jit(opts.version)
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
        repl_as_lua_c(interp);
    } else if args.script.is_none() && !args.has_e && !args.has_v {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            repl_as_lua_c(interp);
        } else {
            // lua.c ignores how this ends: an error in it is reported, and
            // the exit status stays 0
            interp.dofile(None);
        }
    }
    true
}

/// The REPL runs without a program name on its messages, as `lua.c`'s.
fn repl_as_lua_c(interp: &mut Interp) {
    let progname = interp.progname.take();
    repl(&mut interp.vm);
    interp.progname = progname;
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let (opts, argv) = take_luna_opts(argv);
    let progname = match argv.first() {
        Some(p) if !p.is_empty() => p.clone(),
        _ => "lua".to_string(),
    };
    let args = match collectargs(opts.version, &argv) {
        Ok(args) => args,
        Err(bad) => {
            print_usage(opts.version, &progname, &argv[bad]);
            std::process::exit(1);
        }
    };
    let mut interp = Interp {
        vm: new_vm(&opts),
        progname: Some(progname),
    };
    let ok = pmain(&mut interp, &argv, &args);
    if opts.profile {
        print_profile(&interp.vm);
    }
    // lua.c closes the state before it exits, which finalizes open files
    // and so writes out what they still buffer
    drop(interp);
    std::process::exit(if ok { 0 } else { 1 });
}
