//! Lua pattern matching engine — a port of lstrlib.c's matcher.
//! Pure functions over byte slices (stone candidate: no runtime types).
//!
//! The matcher is the 5.2+ one (explicit pattern end, `matchdepth` bound);
//! `Flavor` carries the few places where older dialects differ. 5.1's
//! NUL-terminated patterns are the caller's business: it passes the pattern
//! cut at the first NUL.

const MAX_CAPTURES: usize = 32;
/// PUC `MAXCCALLS`: nested `match` calls allowed before "pattern too complex".
const MAXCCALLS: u32 = 200;
/// 5.1 has no `matchdepth`; its matcher recurses until the C stack runs out.
/// This bound stands in for that stack so a runaway pattern raises instead
/// of overflowing ours.
const MAXCCALLS_51: u32 = 5000;

/// One capture produced by a successful pattern match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cap {
    /// captured span [start, end) in source bytes
    Span(usize, usize),
    /// position capture `()` — byte offset (0-based; callers add 1)
    Pos(usize),
}

/// Error returned by the pattern matcher (malformed pattern, runaway depth,
/// invalid `%f` frontier, etc.).
#[derive(Debug)]
pub struct PatError(
    /// Human-readable message describing the malformation.
    pub String,
);

/// A successful match against a Lua pattern, with the captures it produced.
pub struct Match {
    /// whole-match span [start, end)
    pub start: usize,
    /// End offset of the whole match (exclusive).
    pub end: usize,
    /// Captured spans / positions, in pattern order.
    pub caps: Vec<Cap>,
}

/// Where the dialects' matchers differ.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum Flavor {
    /// No `%g` class, capture-index errors without the index, "unbalanced
    /// pattern" for a short `%b`, no `matchdepth`.
    Lua51,
    /// Numbered capture-index errors while matching, unnumbered ones when a
    /// capture is fetched.
    Lua52,
    /// 5.3 onwards.
    Lua53,
}

const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;

/// A capture as `get_onecapture` sees it once a match succeeded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CapValue {
    Span(usize, usize),
    Pos(usize),
}

/// PUC `MatchState`: the subject, the pattern, and the captures of the match
/// in progress. `try_at` is `reprepstate` + `match`.
pub(crate) struct MatchState<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    flavor: Flavor,
    level: usize,
    capture: [(usize, isize); MAX_CAPTURES],
    matchdepth: u32,
}

fn err<T>(msg: &str) -> Result<T, PatError> {
    Err(PatError(msg.to_string()))
}

impl<'a> MatchState<'a> {
    pub(crate) fn new(src: &'a [u8], pat: &'a [u8], flavor: Flavor) -> Self {
        MatchState {
            src,
            pat,
            flavor,
            level: 0,
            capture: [(0, 0); MAX_CAPTURES],
            matchdepth: if flavor == Flavor::Lua51 {
                MAXCCALLS_51
            } else {
                MAXCCALLS
            },
        }
    }

    /// Match the whole pattern at exactly `s`; `Some(end)` on success.
    pub(crate) fn try_at(&mut self, s: usize) -> Result<Option<usize>, PatError> {
        self.level = 0;
        self.do_match(s, 0)
    }

    /// Number of captures of the last successful match.
    pub(crate) fn level(&self) -> usize {
        self.level
    }

    /// PUC `get_onecapture`: capture `i` of a match spanning `[s, e)`; with
    /// no captures, index 0 is the whole match.
    pub(crate) fn get_capture(&self, i: usize, s: usize, e: usize) -> Result<CapValue, PatError> {
        if i >= self.level {
            if i != 0 {
                return match self.flavor {
                    Flavor::Lua53 => Err(PatError(format!("invalid capture index %{}", i + 1))),
                    _ => err("invalid capture index"),
                };
            }
            return Ok(CapValue::Span(s, e));
        }
        let (init, len) = self.capture[i];
        match len {
            CAP_UNFINISHED => err("unfinished capture"),
            CAP_POSITION => Ok(CapValue::Pos(init)),
            _ => Ok(CapValue::Span(init, init + len as usize)),
        }
    }

    fn do_match(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatError> {
        if self.matchdepth == 0 {
            return err("pattern too complex");
        }
        self.matchdepth -= 1;
        let r = self.match_body(s, p);
        self.matchdepth += 1;
        r
    }

    fn match_body(&mut self, mut s: usize, mut p: usize) -> Result<Option<usize>, PatError> {
        let pat = self.pat;
        loop {
            if p == pat.len() {
                return Ok(Some(s));
            }
            match pat[p] {
                b'(' => {
                    return if pat.get(p + 1) == Some(&b')') {
                        self.start_capture(s, p + 2, CAP_POSITION)
                    } else {
                        self.start_capture(s, p + 1, CAP_UNFINISHED)
                    };
                }
                b')' => return self.end_capture(s, p + 1),
                b'$' if p + 1 == pat.len() => {
                    return Ok((s == self.src.len()).then_some(s));
                }
                b'%' => match pat.get(p + 1) {
                    Some(b'b') => match self.match_balance(s, p + 2)? {
                        Some(ns) => {
                            s = ns;
                            p += 4;
                            continue;
                        }
                        None => return Ok(None),
                    },
                    Some(b'f') => {
                        p += 2;
                        if pat.get(p) != Some(&b'[') {
                            return err("missing '[' after '%f' in pattern");
                        }
                        let ep = self.class_end(p)?;
                        let prev = if s == 0 { 0 } else { self.src[s - 1] };
                        // PUC reads the subject's terminating NUL at its end
                        let cur = self.src.get(s).copied().unwrap_or(0);
                        if !self.match_bracket(prev, p, ep - 1)
                            && self.match_bracket(cur, p, ep - 1)
                        {
                            p = ep;
                            continue;
                        }
                        return Ok(None);
                    }
                    Some(&d) if d.is_ascii_digit() => match self.match_capture(s, d)? {
                        Some(ns) => {
                            s = ns;
                            p += 2;
                            continue;
                        }
                        None => return Ok(None),
                    },
                    _ => {}
                },
                _ => {}
            }
            // a single-char class plus an optional suffix
            let ep = self.class_end(p)?;
            let suffix = pat.get(ep).copied();
            if !self.single_match(s, p, ep) {
                if matches!(suffix, Some(b'*' | b'?' | b'-')) {
                    p = ep + 1;
                    continue;
                }
                return Ok(None);
            }
            match suffix {
                Some(b'?') => {
                    if let Some(r) = self.do_match(s + 1, ep + 1)? {
                        return Ok(Some(r));
                    }
                    p = ep + 1;
                }
                Some(b'+') => return self.max_expand(s + 1, p, ep),
                Some(b'*') => return self.max_expand(s, p, ep),
                Some(b'-') => return self.min_expand(s, p, ep),
                _ => {
                    s += 1;
                    p = ep;
                }
            }
        }
    }

    fn class_end(&self, p: usize) -> Result<usize, PatError> {
        let pat = self.pat;
        match pat[p] {
            b'%' => {
                if p + 1 == pat.len() {
                    return err("malformed pattern (ends with '%')");
                }
                Ok(p + 2)
            }
            b'[' => {
                let mut q = p + 1;
                if pat.get(q) == Some(&b'^') {
                    q += 1;
                }
                // do-while: the first byte is consumed before looking for
                // ']', so "[]" and "[^]" start a set containing ']'
                loop {
                    if q == pat.len() {
                        return err("malformed pattern (missing ']')");
                    }
                    let c = pat[q];
                    q += 1;
                    if c == b'%' && q < pat.len() {
                        q += 1;
                    }
                    if pat.get(q) == Some(&b']') {
                        return Ok(q + 1);
                    }
                }
            }
            _ => Ok(p + 1),
        }
    }

    fn match_class(&self, c: u8, cl: u8) -> bool {
        let res = match cl.to_ascii_lowercase() {
            b'a' => c.is_ascii_alphabetic(),
            b'c' => c.is_ascii_control(),
            b'd' => c.is_ascii_digit(),
            b'g' if self.flavor >= Flavor::Lua52 => c.is_ascii_graphic(),
            b'l' => c.is_ascii_lowercase(),
            b'p' => c.is_ascii_punctuation(),
            b's' => matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r'),
            b'u' => c.is_ascii_uppercase(),
            b'w' => c.is_ascii_alphanumeric(),
            b'x' => c.is_ascii_hexdigit(),
            b'z' => c == 0,
            _ => return cl == c,
        };
        if cl.is_ascii_lowercase() { res } else { !res }
    }

    /// `[set]` test; `p` is the '[' and `ec` the closing ']'.
    fn match_bracket(&self, c: u8, mut p: usize, ec: usize) -> bool {
        let pat = self.pat;
        let mut sig = true;
        if pat[p + 1] == b'^' {
            sig = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= ec {
                return !sig;
            }
            if pat[p] == b'%' {
                p += 1;
                if self.match_class(c, pat[p]) {
                    return sig;
                }
            } else if pat[p + 1] == b'-' && p + 2 < ec {
                p += 2;
                if pat[p - 2] <= c && c <= pat[p] {
                    return sig;
                }
            } else if pat[p] == c {
                return sig;
            }
        }
    }

    fn single_match(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        match self.pat[p] {
            b'.' => true,
            b'%' => self.match_class(c, self.pat[p + 1]),
            b'[' => self.match_bracket(c, p, ep - 1),
            pc => pc == c,
        }
    }

    fn match_balance(&self, s: usize, p: usize) -> Result<Option<usize>, PatError> {
        if p + 1 >= self.pat.len() {
            return if self.flavor == Flavor::Lua51 {
                err("unbalanced pattern")
            } else {
                err("malformed pattern (missing arguments to '%b')")
            };
        }
        let (b, e) = (self.pat[p], self.pat[p + 1]);
        if self.src.get(s) != Some(&b) {
            return Ok(None);
        }
        let mut cont = 1;
        for (i, &c) in self.src.iter().enumerate().skip(s + 1) {
            if c == e {
                cont -= 1;
                if cont == 0 {
                    return Ok(Some(i + 1));
                }
            } else if c == b {
                cont += 1;
            }
        }
        Ok(None)
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> Result<Option<usize>, PatError> {
        let mut i = 0;
        while self.single_match(s + i, p, ep) {
            i += 1;
        }
        loop {
            if let Some(r) = self.do_match(s + i, ep + 1)? {
                return Ok(Some(r));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> Result<Option<usize>, PatError> {
        loop {
            if let Some(r) = self.do_match(s, ep + 1)? {
                return Ok(Some(r));
            }
            if self.single_match(s, p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    fn start_capture(
        &mut self,
        s: usize,
        p: usize,
        what: isize,
    ) -> Result<Option<usize>, PatError> {
        if self.level >= MAX_CAPTURES {
            return err("too many captures");
        }
        self.capture[self.level] = (s, what);
        self.level += 1;
        let r = self.do_match(s, p)?;
        if r.is_none() {
            self.level -= 1;
        }
        Ok(r)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> Result<Option<usize>, PatError> {
        let l = self.capture_to_close()?;
        self.capture[l].1 = (s - self.capture[l].0) as isize;
        let r = self.do_match(s, p)?;
        if r.is_none() {
            self.capture[l].1 = CAP_UNFINISHED;
        }
        Ok(r)
    }

    fn capture_to_close(&self) -> Result<usize, PatError> {
        (0..self.level)
            .rev()
            .find(|&l| self.capture[l].1 == CAP_UNFINISHED)
            .map_or_else(|| err("invalid pattern capture"), Ok)
    }

    /// Back-reference `%d`. A position capture passes the index check and
    /// then never matches: its length is the (huge) `CAP_POSITION` cast.
    fn match_capture(&self, s: usize, d: u8) -> Result<Option<usize>, PatError> {
        let l = d as isize - b'1' as isize;
        if l < 0 || l as usize >= self.level || self.capture[l as usize].1 == CAP_UNFINISHED {
            return if self.flavor == Flavor::Lua51 {
                err("invalid capture index")
            } else {
                Err(PatError(format!("invalid capture index %{}", l + 1)))
            };
        }
        let (init, len) = self.capture[l as usize];
        let len = len as usize;
        if self.src.len() - s >= len && self.src[init..init + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }
}

/// Split a leading `^` anchor from the pattern body. The caller decides what
/// the anchor means (find/match scan at most once; gsub stops after the first
/// position).
pub fn anchor_split(pat: &[u8]) -> (bool, &[u8]) {
    match pat.first() {
        Some(b'^') => (true, &pat[1..]),
        _ => (false, pat),
    }
}

/// Try to match `pat_body` (already `^`-stripped) at exactly position `s`,
/// with no forward scan. Returns the Match (whose `start == s`) or None; a
/// capture left open by a successful match is an error.
pub fn match_at(src: &[u8], pat_body: &[u8], s: usize) -> Result<Option<Match>, PatError> {
    let mut ms = MatchState::new(src, pat_body, Flavor::Lua53);
    let Some(e) = ms.try_at(s)? else {
        return Ok(None);
    };
    let caps = (0..ms.level())
        .map(|i| {
            ms.get_capture(i, s, e).map(|c| match c {
                CapValue::Span(a, b) => Cap::Span(a, b),
                CapValue::Pos(p) => Cap::Pos(p),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Match {
        start: s,
        end: e,
        caps,
    }))
}

/// Scan from `init` for the first match (PUC str_find_aux without the plain
/// fast path). A leading `^` anchors the search to `init`.
pub fn find(src: &[u8], pat: &[u8], init: usize) -> Result<Option<Match>, PatError> {
    if init > src.len() {
        return Ok(None);
    }
    let (anchor, pat_body) = anchor_split(pat);
    let mut s = init;
    loop {
        if let Some(m) = match_at(src, pat_body, s)? {
            return Ok(Some(m));
        }
        if anchor || s >= src.len() {
            return Ok(None);
        }
        s += 1;
    }
}

/// Whether the pattern contains a byte from PUC's `SPECIALS`; a pattern
/// without one is searched for as plain text.
pub fn has_specials(pat: &[u8]) -> bool {
    pat.iter().any(|c| {
        matches!(
            c,
            b'^' | b'$' | b'*' | b'+' | b'?' | b'.' | b'(' | b'[' | b'%' | b'-'
        )
    })
}

/// Plain substring search (find with plain=true).
pub fn plain_find(hay: &[u8], needle: &[u8], init: usize) -> Option<usize> {
    if init > hay.len() {
        return None;
    }
    if needle.is_empty() {
        return Some(init);
    }
    hay[init..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + init)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(src: &str, pat: &str) -> Option<(usize, usize)> {
        find(src.as_bytes(), pat.as_bytes(), 0)
            .unwrap()
            .map(|m| (m.start, m.end))
    }

    #[test]
    fn basics() {
        assert_eq!(m("hello", "l+"), Some((2, 4)));
        assert_eq!(m("hello", "^h"), Some((0, 1)));
        assert_eq!(m("hello", "^e"), None);
        assert_eq!(m("hello", "o$"), Some((4, 5)));
        assert_eq!(m("hello", "%a+"), Some((0, 5)));
        assert_eq!(m("a1b2", "%d"), Some((1, 2)));
        assert_eq!(m("abc", "a.c"), Some((0, 3)));
        assert_eq!(m("", ".*"), Some((0, 0)));
        assert_eq!(m("abc", "x*"), Some((0, 0)));
    }

    #[test]
    fn sets_and_quantifiers() {
        assert_eq!(m("hello world", "[aeiou]"), Some((1, 2)));
        assert_eq!(m("hello", "[^aeiou]+"), Some((0, 1)));
        assert_eq!(m("x123y", "[0-9]+"), Some((1, 4)));
        assert_eq!(m("aaa", "a-"), Some((0, 0)));
        assert_eq!(m("<a><b>", "<.->"), Some((0, 3)));
        assert_eq!(m("<a><b>", "<.*>"), Some((0, 6)));
        assert_eq!(m("abc", "ab?c"), Some((0, 3)));
        assert_eq!(m("ac", "ab?c"), Some((0, 2)));
    }

    #[test]
    fn captures_and_specials() {
        let mm = find(b"key=value", b"(%w+)=(%w+)", 0).unwrap().unwrap();
        assert_eq!(mm.caps.len(), 2);
        assert_eq!(mm.caps[0], Cap::Span(0, 3));
        assert_eq!(mm.caps[1], Cap::Span(4, 9));
        // position capture
        let mm = find(b"abc", b"a()b", 0).unwrap().unwrap();
        assert_eq!(mm.caps[0], Cap::Pos(1));
        // balanced
        assert_eq!(m("(foo(bar))baz", "%b()"), Some((0, 10)));
        // frontier
        assert_eq!(m("THE (quick) fox", "%f[%a]%a+"), Some((0, 3)));
        // back-reference
        assert_eq!(m("abcabc", "(abc)%1"), Some((0, 6)));
        assert_eq!(m("abcabd", "(abc)%1"), None);
    }

    #[test]
    fn errors() {
        assert!(find(b"x", b"%", 0).is_err());
        assert!(find(b"x", b"[abc", 0).is_err());
        assert!(find(b"a", b"(a", 0).is_err()); // unfinished capture
        assert!(find(b"x", b"%1", 0).is_err());
    }
}
