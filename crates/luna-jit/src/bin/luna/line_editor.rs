//! The REPL's line editor on a terminal (the `repl-line-editor` feature),
//! in the place of the readline library lua.c can be built with: tab
//! completion against the `Vm`'s globals, Lua syntax highlighting, and a
//! history kept in `~/.luna_history`.

use crate::repl::Line;
use luna_jit::runtime::Value;
use luna_jit::vm::Vm;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

pub(crate) struct LineEditor {
    rl: rustyline::Editor<LuaHelper, DefaultHistory>,
    globals: Rc<RefCell<GlobalsSnapshot>>,
    history: Option<PathBuf>,
}

impl LineEditor {
    pub(crate) fn new() -> rustyline::Result<LineEditor> {
        let globals = Rc::new(RefCell::new(GlobalsSnapshot::default()));
        let mut rl = rustyline::Editor::new()?;
        rl.set_helper(Some(LuaHelper {
            globals: globals.clone(),
        }));
        let history = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".luna_history"));
        if let Some(p) = &history {
            // a missing or unreadable history file starts an empty one
            let _ = rl.load_history(p);
        }
        Ok(LineEditor {
            rl,
            globals,
            history,
        })
    }

    /// `readline(prompt)`.
    pub(crate) fn read(&mut self, vm: &mut Vm, prompt: &[u8]) -> Line {
        refresh_globals_snapshot(vm, &self.globals);
        match self.rl.readline(&*String::from_utf8_lossy(prompt)) {
            Ok(l) => Line::Text(l.into_bytes()),
            // Ctrl-C drops the statement being typed
            Err(ReadlineError::Interrupted) => Line::Cancel,
            Err(_) => Line::Eof,
        }
    }

    /// `add_history`.
    pub(crate) fn save(&mut self, line: &[u8]) {
        let _ = self.rl.add_history_entry(String::from_utf8_lossy(line));
    }

    pub(crate) fn finish(mut self) {
        if let Some(p) = &self.history {
            // the session itself is over; a history that cannot be written
            // is only lost
            let _ = self.rl.save_history(p);
        }
    }
}

#[derive(Default)]
struct GlobalsSnapshot {
    names: Vec<String>,
}

struct LuaHelper {
    globals: std::rc::Rc<std::cell::RefCell<GlobalsSnapshot>>,
}

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

impl rustyline::hint::Hinter for LuaHelper {
    type Hint = String;
}

impl rustyline::validate::Validator for LuaHelper {}

impl rustyline::Helper for LuaHelper {}

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
    fn highlight_char(
        &self,
        _line: &str,
        _pos: usize,
        _kind: rustyline::highlight::CmdKind,
    ) -> bool {
        // Re-render every keystroke — tokenizer is cheap and partial
        // highlights look broken mid-string / mid-comment.
        true
    }
}

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
