//! `lua.c`'s read-eval-print loop of each dialect: `dotty` (5.1, 5.2) and
//! `doREPL` (5.3 on), with the line reading of a build without readline —
//! the prompt on stdout, then `fgets` on stdin — or the line editor on a
//! terminal when it is built in.

use crate::{Interp, lua_tostring};
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use std::io::Write;

/// `LUA_MAXINPUT`: the size of `lua.c`'s line buffer.
const MAXINPUT: usize = 512;

/// A line as the reader gave it.
pub(crate) enum Line {
    Text(Vec<u8>),
    Eof,
    /// the line editor's Ctrl-C: drop the statement being typed
    #[cfg(feature = "repl-line-editor")]
    Cancel,
}

enum Input {
    Stdin,
    #[cfg(feature = "repl-line-editor")]
    Editor(crate::line_editor::LineEditor),
}

impl Input {
    fn new() -> Input {
        #[cfg(feature = "repl-line-editor")]
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            match crate::line_editor::LineEditor::new() {
                Ok(ed) => return Input::Editor(ed),
                Err(e) => eprintln!("line editor unavailable ({e}); reading plain lines"),
            }
        }
        Input::Stdin
    }

    /// `lua_readline`.
    fn read(&mut self, interp: &mut Interp, prompt: &[u8]) -> Line {
        match self {
            Input::Stdin => {
                let mut out = std::io::stdout().lock();
                // unchecked, as lua.c's fputs / fflush
                let _ = out.write_all(prompt);
                let _ = out.flush();
                drop(out);
                match interp.vm.read_stdin_line(MAXINPUT) {
                    Ok(Some(l)) => Line::Text(l),
                    // fgets gives NULL at the end of input and on an error
                    Ok(None) | Err(_) => Line::Eof,
                }
            }
            #[cfg(feature = "repl-line-editor")]
            Input::Editor(ed) => ed.read(&mut interp.vm, prompt),
        }
    }

    /// `lua_saveline`: a build without readline keeps no history.
    #[cfg_attr(not(feature = "repl-line-editor"), allow(unused_variables))]
    fn save(&mut self, line: &[u8]) {
        #[cfg(feature = "repl-line-editor")]
        if let Input::Editor(ed) = self
            && !line.is_empty()
        {
            ed.save(line);
        }
    }

    fn finish(self) {
        match self {
            Input::Stdin => {}
            #[cfg(feature = "repl-line-editor")]
            Input::Editor(ed) => ed.finish(),
        }
    }
}

/// What `loadline` found.
enum Loaded {
    /// no more input
    Eof,
    /// the compiled chunk, or the error that stopped it compiling
    Chunk(Result<Value, Value>),
    #[cfg(feature = "repl-line-editor")]
    Dropped,
}

/// The REPL, run without a program name on its messages. False when an
/// error escaped it (a `__tostring` of `_PROMPT` failing, 5.4 on), which
/// `lua.c` reports from `main`, also without the program name.
pub(crate) fn repl(interp: &mut Interp) -> bool {
    let progname = interp.progname.take();
    let mut input = Input::new();
    let ok = loop {
        let chunk = match loadline(interp, &mut input) {
            Ok(Loaded::Eof) => break true,
            Ok(Loaded::Chunk(c)) => c,
            #[cfg(feature = "repl-line-editor")]
            Ok(Loaded::Dropped) => continue,
            Err(e) => {
                interp.report(e);
                break false;
            }
        };
        match chunk.and_then(|f| interp.docall(f, &[])) {
            Ok(vals) => l_print(interp, vals),
            Err(e) => interp.report(e),
        }
    };
    input.finish();
    if ok {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
    interp.progname = progname;
    ok
}

/// `loadline`: read a statement, over as many lines as it needs, and
/// compile it.
fn loadline(interp: &mut Interp, input: &mut Input) -> Result<Loaded, Value> {
    let v = interp.version();
    let mut line = match pushline(interp, input, true)? {
        Line::Text(l) => l,
        Line::Eof => return Ok(Loaded::Eof),
        #[cfg(feature = "repl-line-editor")]
        Line::Cancel => return Ok(Loaded::Dropped),
    };
    let loaded = if v <= LuaVersion::Lua52 {
        // no `return` added; input ending mid-statement ends the loop
        loop {
            match load(interp, &line) {
                Err(e) if incomplete(v, e) => {}
                r => break r,
            }
            match pushline(interp, input, false)? {
                Line::Text(more) => join(&mut line, &more),
                Line::Eof => return Ok(Loaded::Eof),
                #[cfg(feature = "repl-line-editor")]
                Line::Cancel => return Ok(Loaded::Dropped),
            }
        }
    } else {
        let mut ret = b"return ".to_vec();
        ret.extend_from_slice(&line);
        ret.push(b';');
        match load(interp, &ret) {
            Ok(f) => Ok(f),
            Err(_) => match multiline(interp, input, &mut line)? {
                Some(r) => r,
                #[cfg(feature = "repl-line-editor")]
                None => return Ok(Loaded::Dropped),
                #[cfg(not(feature = "repl-line-editor"))]
                None => unreachable!("only the line editor drops a statement"),
            },
        }
    };
    input.save(&line);
    Ok(Loaded::Chunk(loaded))
}

/// 5.3 on: compile `line` as a statement, reading more lines while it is
/// incomplete; at the end of input the incomplete statement's error
/// stands, except in 5.3, which has popped it and reports what is left
/// on its stack instead: the `_PROMPT2` it fetched for the prompt. `None`
/// when the line editor dropped the statement.
fn multiline(
    interp: &mut Interp,
    input: &mut Input,
    line: &mut Vec<u8>,
) -> Result<Option<Result<Value, Value>>, Value> {
    let v = interp.version();
    if v >= LuaVersion::Lua55 {
        checklocal(line);
    }
    loop {
        let r = load(interp, line);
        match r {
            Err(e) if incomplete(v, e) => {}
            r => return Ok(Some(r)),
        }
        match pushline(interp, input, false)? {
            Line::Text(more) => join(line, &more),
            Line::Eof if v == LuaVersion::Lua53 => {
                let key = interp.str_value("_PROMPT2");
                return Ok(Some(Err(interp.vm.globals().get(key))));
            }
            Line::Eof => return Ok(Some(r)),
            #[cfg(feature = "repl-line-editor")]
            Line::Cancel => return Ok(None),
        }
    }
}

fn join(line: &mut Vec<u8>, more: &[u8]) {
    line.push(b'\n');
    line.extend_from_slice(more);
}

/// Compile a line as the chunk `stdin`; 5.5 takes text only.
fn load(interp: &mut Interp, src: &[u8]) -> Result<Value, Value> {
    let mode = (interp.version() >= LuaVersion::Lua55).then_some(&b"t"[..]);
    interp.vm.load_buffer(src, b"=stdin", mode).map_err(|e| e.0)
}

/// `incomplete`: a syntax error at the end of the input. 5.1 looks for its
/// quoted `'<eof>'` (its first occurrence) at the end of the message.
fn incomplete(v: LuaVersion, err: Value) -> bool {
    let Value::Str(s) = err else { return false };
    let msg = s.as_bytes();
    if v == LuaVersion::Lua51 {
        let mark: &[u8] = b"'<eof>'";
        msg.len() >= mark.len()
            && msg.windows(mark.len()).position(|w| w == mark) == Some(msg.len() - mark.len())
    } else {
        msg.ends_with(b"<eof>")
    }
}

/// `pushline`: prompt, read a line and take its newline off. Through 5.4 a
/// first line `=expr` stands for `return expr`.
fn pushline(interp: &mut Interp, input: &mut Input, firstline: bool) -> Result<Line, Value> {
    let prompt = get_prompt(interp, firstline)?;
    let mut b = match input.read(interp, &prompt) {
        Line::Text(b) => b,
        other => return Ok(other),
    };
    // lua.c takes the line as a C string, up to its first NUL
    if let Some(nul) = b.iter().position(|&c| c == 0) {
        b.truncate(nul);
    }
    if b.last() == Some(&b'\n') {
        b.pop();
    }
    if firstline && interp.version() <= LuaVersion::Lua54 && b.first() == Some(&b'=') {
        let mut ret = b"return ".to_vec();
        ret.extend_from_slice(&b[1..]);
        b = ret;
    }
    Ok(Line::Text(b))
}

/// `get_prompt`: the global `_PROMPT` / `_PROMPT2` when it converts to a
/// string — 5.1 to 5.3 take a string or a number, 5.4 on anything but nil,
/// through `__tostring` — else `> ` / `>> `. Printed as a C string.
fn get_prompt(interp: &mut Interp, firstline: bool) -> Result<Vec<u8>, Value> {
    let (name, default): (&str, &[u8]) = if firstline {
        ("_PROMPT", b"> ")
    } else {
        ("_PROMPT2", b">> ")
    };
    let key = interp.str_value(name);
    let val = interp.vm.globals().get(key);
    let mut p = if interp.version() <= LuaVersion::Lua53 {
        lua_tostring(&mut interp.vm, val).unwrap_or_else(|| default.to_vec())
    } else if val.is_nil() {
        default.to_vec()
    } else {
        interp.vm.tostring_value(val).map_err(|e| e.0)?
    };
    if let Some(nul) = p.iter().position(|&c| c == 0) {
        p.truncate(nul);
    }
    Ok(p)
}

/// 5.5's `checklocal`: a line starting with `local` warns that the local
/// ends with the line.
fn checklocal(line: &[u8]) {
    let rest = &line[line
        .iter()
        .take_while(|&&c| c == b' ' || c == b'\t')
        .count()..];
    // strchr(" \t", c) also finds the string's terminating NUL
    if rest.starts_with(b"local") && matches!(rest.get(5), None | Some(b' ' | b'\t')) {
        eprintln!("warning: locals do not survive across lines in interactive mode");
    }
}

/// `l_print`: the chunk's results through the global `print`, called
/// unprotected by a message handler.
fn l_print(interp: &mut Interp, vals: Vec<Value>) {
    if vals.is_empty() {
        return;
    }
    let key = interp.str_value("print");
    let print = interp.vm.globals().get(key);
    if let Err(e) = interp.vm.call_value(print, &vals) {
        // lua_pushfstring renders a NULL string as "(null)"
        let err = lua_tostring(&mut interp.vm, e.0).unwrap_or_else(|| b"(null)".to_vec());
        let mut msg = b"error calling 'print' (".to_vec();
        msg.extend_from_slice(&err);
        msg.push(b')');
        interp.message(&msg);
    }
}
