//! The strings PUC makes raising a syntax error: the message its parser
//! formats (with the tokens it names, `luaX_token2str`), the chunk and line
//! put before it (`luaG_addinfo`), the near-token (`txtToken`), and the two
//! joined. luna's message has PUC's bytes, so the pieces are read back from
//! it.

use super::*;
use crate::frontend::error::NearKind;
use crate::runtime::puc_fmt::FmtArg;
use crate::version::LuaVersion;

/// The messages PUC's parser formats with arguments, by dialect, besides
/// those naming tokens (`error_expected`, `check_match`). `%W` is
/// `errorlimit`'s function, itself formatted when it is not the main one.
const FORMATS_51: &[&str] = &[
    "main function has more than %d %s",
    "function at line %d has more than %d %s",
];
const FORMATS_52: &[&str] = &[
    "too many %s (limit is %d) in %W",
    "<%s> at line %d not inside a loop",
    "no visible label '%s' for <goto> at line %d",
    "<goto %s> at line %d jumps into the scope of local '%s'",
    "label '%s' already defined on line %d",
];
const FORMATS_54: &[&str] = &[
    "too many %s (limit is %d) in %W",
    "break outside loop at line %d",
    "no visible label '%s' for <goto> at line %d",
    "<goto %s> at line %d jumps into the scope of local '%s'",
    "label '%s' already defined on line %d",
    "attempt to assign to const variable '%s'",
    "unknown attribute '%s'",
];
const FORMATS_55: &[&str] = &[
    "too many %s (limit is %d) in %W",
    "attempt to assign to const variable '%s'",
    "%s is global when accessing variable '%s'",
    "variable '%s' not declared",
    "<goto %s> at line %d jumps into the scope of '%s'",
    "no visible label '%s' for <goto> at line %d",
    "label '%s' already defined on line %d",
    "unknown attribute '%s'",
    "multiple to-be-closed variables in local list",
    "global variables cannot be to-be-closed",
];

/// The scanner's formatted message, 5.3 on (`read_long_string`).
const FORMATS_LEX_53: &[&str] = &["unfinished long %s (starting at line %d)"];

/// The arguments `fmt` (with `%s`, `%d`, `%W`) formatted to make `text`.
fn matched<'t>(fmt: &[u8], text: &'t [u8], out: &mut Vec<&'t [u8]>) -> bool {
    match fmt {
        [] => text.is_empty(),
        [b'%', d @ (b's' | b'd' | b'W'), rest @ ..] => {
            let digits = *d == b'd';
            for end in (0..=text.len()).rev() {
                let arg = &text[..end];
                if digits && (arg.is_empty() || !arg.iter().all(u8::is_ascii_digit)) {
                    continue;
                }
                out.push(arg);
                if matched(rest, &text[end..], out) {
                    return true;
                }
                out.pop();
            }
            false
        }
        [c, rest @ ..] => text.first() == Some(c) && matched(rest, &text[1..], out),
    }
}

fn number(b: &[u8]) -> i64 {
    std::str::from_utf8(b)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

impl Vm {
    /// PUC's strings for the syntax error `e` of the chunk `source`, whose
    /// near-token is `near`.
    pub(super) fn replay_syntax_error(
        &mut self,
        r: &mut Replay,
        e: &SyntaxError,
        near: NearKind,
        source: &[u8],
    ) -> Result<(), SyntaxError> {
        let v = self.version;
        if e.line == 0 {
            // a runtime error raised by `load`, not positioned
            let m = self.heap.pushfstring(&e.msg, &[])?;
            self.heap.puc.load_msg = Some(m);
            return Ok(());
        }
        let (inner, near_text) = match near {
            NearKind::None => (&e.msg[..], &b""[..]),
            _ => match e.msg.windows(6).rposition(|w| w == b" near ") {
                Some(i) => (&e.msg[..i], &e.msg[i + 6..]),
                None => (&e.msg[..], &b""[..]),
            },
        };
        self.parser_msg(inner)?;
        let id = crate::vm::callstack::syntax_chunk_id(v, source);
        let line = FmtArg::D(i64::from(e.line));
        let msg = self
            .heap
            .pushfstring(b"%s:%d: %s", &[FmtArg::S(&id), line, FmtArg::S(inner)])?;
        if near == NearKind::None {
            self.heap.puc.load_msg = Some(msg);
            return Ok(());
        }
        let msg = msg.as_bytes().to_vec();
        let unquoted = near_text
            .strip_prefix(b"'")
            .and_then(|t| t.strip_suffix(b"'"))
            .unwrap_or(near_text);
        match near {
            NearKind::Text | NearKind::Numeral => {
                // `save(ls, '\0')` after the token's bytes (a numeral's
                // buffer holds its terminating zero already)
                let n = unquoted.len() + usize::from(near == NearKind::Numeral);
                self.grow_buff(r, n + 1);
                if v != LuaVersion::Lua51 {
                    self.heap.pushfstring(b"'%s'", &[FmtArg::S(unquoted)])?;
                }
            }
            // 5.1 spells a one-byte token with `%c` as it does any other
            NearKind::Char => self.char_token(unquoted)?,
            NearKind::Fixed if v == LuaVersion::Lua51 && unquoted.len() == 1 => {
                self.char_token(unquoted)?;
            }
            NearKind::Fixed if v != LuaVersion::Lua51 => {
                self.heap.pushfstring(b"'%s'", &[FmtArg::S(unquoted)])?;
            }
            _ => {}
        }
        let fmt: &[u8] = if v == LuaVersion::Lua51 {
            b"%s near '%s'"
        } else {
            b"%s near %s"
        };
        let tok = if v == LuaVersion::Lua51 {
            unquoted
        } else {
            near_text
        };
        let m = self
            .heap
            .pushfstring(fmt, &[FmtArg::S(&msg), FmtArg::S(tok)])?;
        self.heap.puc.load_msg = Some(m);
        Ok(())
    }

    /// The strings PUC's parser makes formatting `msg`, if it formats it.
    fn parser_msg(&mut self, msg: &[u8]) -> Result<(), SyntaxError> {
        let v = self.version;
        if let Some(x) = msg.strip_suffix(b" expected") {
            if let Some(t) = token_named(v, x) {
                // `error_expected`
                let t = self.token2str(t)?;
                let fmt: &[u8] = if v == LuaVersion::Lua51 {
                    b"'%s' expected"
                } else {
                    b"%s expected"
                };
                self.heap.pushfstring(fmt, &[FmtArg::S(&t)])?;
                return Ok(());
            }
        }
        let mut args = Vec::new();
        let close: &[u8] = if v == LuaVersion::Lua51 {
            b"'%s' expected (to close '%s' at line %d)"
        } else {
            b"%s expected (to close %s at line %d)"
        };
        if matched(close, msg, &mut args)
            && let (Some(what), Some(who)) = (token_named(v, args[0]), token_named(v, args[1]))
        {
            // `check_match`: the arguments are evaluated last to first
            let who = self.token2str(who)?;
            let what = self.token2str(what)?;
            let line = FmtArg::D(number(args[2]));
            self.heap
                .pushfstring(close, &[FmtArg::S(&what), FmtArg::S(&who), line])?;
            return Ok(());
        }
        let formats = match v {
            LuaVersion::Lua51 => FORMATS_51,
            LuaVersion::Lua52 | LuaVersion::Lua53 => FORMATS_52,
            LuaVersion::Lua54 => FORMATS_54,
            _ => FORMATS_55,
        };
        let lex = if v >= LuaVersion::Lua53 {
            FORMATS_LEX_53
        } else {
            &[]
        };
        for fmt in formats.iter().chain(lex) {
            args.clear();
            if !matched(fmt.as_bytes(), msg, &mut args) {
                continue;
            }
            let fmt = fmt.replace("%W", "%s");
            let mut fa = Vec::with_capacity(args.len());
            let mut d = fmt
                .as_bytes()
                .windows(2)
                .filter(|w| w[0] == b'%')
                .map(|w| w[1]);
            for a in &args {
                match d.next() {
                    Some(b'd') => fa.push(FmtArg::D(number(a))),
                    _ => fa.push(FmtArg::S(a)),
                }
            }
            if let Some(line) = args
                .last()
                .and_then(|w| w.strip_prefix(b"function at line "))
            {
                // `errorlimit`'s function
                let l = FmtArg::D(number(line));
                self.heap.pushfstring(b"function at line %d", &[l])?;
            }
            self.heap.pushfstring(fmt.as_bytes(), &fa)?;
            return Ok(());
        }
        Ok(())
    }

    /// `luaX_token2str` of the token PUC spells `t`: the string it makes,
    /// if any, and the text it gives.
    fn token2str(&mut self, t: &[u8]) -> Result<Vec<u8>, SyntaxError> {
        let v = self.version;
        if t.starts_with(b"<") && t.len() > 2 || v == LuaVersion::Lua51 && t.len() > 1 {
            return Ok(t.to_vec());
        }
        if t.len() == 1 {
            self.char_token(t)?;
        } else {
            self.heap.pushfstring(b"'%s'", &[FmtArg::S(t)])?;
        }
        Ok(if v == LuaVersion::Lua51 {
            t.to_vec()
        } else {
            [b"'", t, b"'"].concat()
        })
    }

    /// `luaX_token2str` of a single-byte token, shown as `t`.
    fn char_token(&mut self, t: &[u8]) -> Result<(), SyntaxError> {
        let v = self.version;
        let code = |p: &[u8], s: &[u8]| {
            t.strip_prefix(p)
                .and_then(|r| r.strip_suffix(s))
                .map(number)
        };
        let (fmt, arg): (&[u8], FmtArg) = match (code(b"char(", b")"), code(b"<\\", b">")) {
            (Some(n), _) => (b"char(%d)", FmtArg::D(n)),
            (_, Some(n)) => (b"'<\\%d>'", FmtArg::D(n)),
            _ if v == LuaVersion::Lua51 => (b"%c", FmtArg::C(t.first().copied().unwrap_or(0))),
            _ => (b"'%c'", FmtArg::C(t.first().copied().unwrap_or(0))),
        };
        self.heap.pushfstring(fmt, &[arg])?;
        Ok(())
    }
}

/// The token `x` names, as a message shows it: quoted (5.1 every one, the
/// later dialects the fixed ones), or `<eof>`, `<name>` and the like.
fn token_named(v: LuaVersion, x: &[u8]) -> Option<&[u8]> {
    if let Some(t) = x.strip_prefix(b"'").and_then(|t| t.strip_suffix(b"'")) {
        return (!t.is_empty() && !t.iter().any(|&c| c == b'\'' || c == b' ')).then_some(t);
    }
    let bare = v != LuaVersion::Lua51
        && x.len() > 2
        && x[0] == b'<'
        && x[x.len() - 1] == b'>'
        && x[1..x.len() - 1].iter().all(u8::is_ascii_lowercase);
    bare.then_some(x)
}
