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

use luna_jit::VmExt; // brings install_default_jit / install_null_jit dotted-method form
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
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
    vm.set_global("arg", Value::Table(t))
        .expect("CLI arg setup");
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
/// etc.). Per R-A1 audit: luna's `SyntaxError::msg` carries "near
/// <eof>" / "unfinished … near <eof>" markers exactly when more input
/// would let the parser continue. The single counter-example is the
/// explicit "'<eof>' expected" form, which means the parser saw EXTRA
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
fn repl(version: LuaVersion) {
    #[cfg(feature = "repl-line-editor")]
    repl_rustyline(version);
    #[cfg(not(feature = "repl-line-editor"))]
    repl_plain(version);
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
fn repl_plain(version: LuaVersion) {
    let mut vm = luna_jit::new_with_jit(version);
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
fn repl_rustyline(version: LuaVersion) {
    use rustyline::Editor;
    use rustyline::error::ReadlineError;
    use rustyline::history::DefaultHistory;
    use std::cell::RefCell;
    use std::rc::Rc;

    let mut vm = luna_jit::new_with_jit(version);
    eprintln!(
        "luna {} ({}) — interactive REPL (rustyline). Ctrl-D to exit.",
        env!("CARGO_PKG_VERSION"),
        match version {
            LuaVersion::Lua51 => "Lua 5.1",
            LuaVersion::Lua52 => "Lua 5.2",
            LuaVersion::Lua53 => "Lua 5.3",
            LuaVersion::Lua54 => "Lua 5.4",
            LuaVersion::Lua55 => "Lua 5.5",
        }
    );

    let globals = Rc::new(RefCell::new(GlobalsSnapshot::default()));
    let helper = LuaHelper {
        globals: globals.clone(),
    };

    let mut rl: Editor<LuaHelper, DefaultHistory> = match Editor::new() {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("rustyline init failed ({e}); falling back to plain REPL");
            repl_plain(version);
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
        refresh_globals_snapshot(&mut vm, &globals);
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
        let mut vm = luna_jit::vm::Vm::sandbox(version)
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
        let mut vm = luna_jit::vm::Vm::new(version);
        vm.install_null_jit();
        vm
    } else {
        luna_jit::new_with_jit(version)
    };

    if let Some(n) = budget {
        vm.set_instr_budget(Some(n));
    }

    populate_arg(&mut vm, script_for_arg.as_deref(), &extra);

    // C5 — pretty error rendering: ANSI color when stderr is a TTY
    // and NO_COLOR isn't set. Embedders piping luna output to logs
    // get plain text automatically.
    let color = std::env::var_os("NO_COLOR").is_none()
        && std::io::IsTerminal::is_terminal(&std::io::stderr());

    let cl = match vm.load(&src, chunkname.as_bytes()) {
        Ok(cl) => cl,
        Err(e) => {
            print_pretty_error(
                &mut vm,
                &format!("{e}"),
                &src,
                color,
                /*compile=*/ true,
            );
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
