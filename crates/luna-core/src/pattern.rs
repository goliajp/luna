//! Lua pattern matching engine — a faithful port of lstrlib.c's matcher.
//! Pure functions over byte slices (stone candidate: no runtime types).

const MAX_CAPTURES: usize = 32;
const MAX_DEPTH: u32 = 220;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cap {
    /// captured span [start, end) in source bytes
    Span(usize, usize),
    /// position capture `()` — byte offset (0-based; callers add 1)
    Pos(usize),
}

#[derive(Debug)]
pub struct PatError(pub String);

pub struct Match {
    /// whole-match span [start, end)
    pub start: usize,
    pub end: usize,
    pub caps: Vec<Cap>,
}

struct State<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    caps: Vec<(usize, isize)>, // (start, len); CAP_UNFINISHED / CAP_POSITION
    depth: u32,
}

const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;

/// Split a leading `^` anchor from the pattern body. The caller decides what
/// the anchor means (find/match scan at most once; gsub/gmatch stop after the
/// first position).
pub fn anchor_split(pat: &[u8]) -> (bool, &[u8]) {
    match pat.first() {
        Some(b'^') => (true, &pat[1..]),
        _ => (false, pat),
    }
}

/// Try to match `pat_body` (already `^`-stripped) at exactly position `s`,
/// with no forward scan. Returns the Match (whose `start == s`) or None.
pub fn match_at(src: &[u8], pat_body: &[u8], s: usize) -> Result<Option<Match>, PatError> {
    let mut st = State {
        src,
        pat: pat_body,
        caps: Vec::new(),
        depth: 0,
    };
    let Some(e) = do_match(&mut st, s, 0)? else {
        return Ok(None);
    };
    if st.caps.iter().any(|&(_, l)| l == CAP_UNFINISHED) {
        return Err(PatError("unfinished capture".into()));
    }
    let caps = st
        .caps
        .iter()
        .map(|&(cs, cl)| {
            if cl == CAP_POSITION {
                Cap::Pos(cs)
            } else {
                Cap::Span(cs, cs + cl as usize)
            }
        })
        .collect();
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

fn class_match(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => c.is_ascii_graphic(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r'),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,      // the \0 class (kept by PUC for compatibility)
        _ => return c == cl, // escaped literal (%%, %., ...)
    };
    if cl.is_ascii_uppercase() { !res } else { res }
}

/// `[set]` matching; `pp` points at the '[' position, `ep` one past ']'.
fn match_bracket(c: u8, pat: &[u8], pp: usize, ep: usize) -> bool {
    let mut p = pp + 1;
    let mut neg = false;
    if pat.get(p) == Some(&b'^') {
        neg = true;
        p += 1;
    }
    let mut found = false;
    while p < ep - 1 {
        if pat[p] == b'%' && p + 1 < ep - 1 {
            p += 1;
            if class_match(c, pat[p]) {
                found = true;
            }
            p += 1;
        } else if p + 2 < ep - 1 && pat[p + 1] == b'-' {
            if pat[p] <= c && c <= pat[p + 2] {
                found = true;
            }
            p += 3;
        } else {
            if pat[p] == c {
                found = true;
            }
            p += 1;
        }
    }
    found != neg
}

/// One past the end of the class starting at `p` (PUC classEnd).
fn class_end(st: &State, p: usize) -> Result<usize, PatError> {
    let pat = st.pat;
    match pat.get(p) {
        None => Err(PatError("malformed pattern (ends with '%')".into())),
        Some(b'%') => {
            if p + 1 >= pat.len() {
                return Err(PatError("malformed pattern (ends with '%')".into()));
            }
            Ok(p + 2)
        }
        Some(b'[') => {
            // PUC classEnd: do-while consumes one char before checking ']',
            // so a ']' right after '[' or '[^' is a literal set member
            let mut q = p + 1;
            if pat.get(q) == Some(&b'^') {
                q += 1;
            }
            loop {
                if q >= pat.len() {
                    return Err(PatError("malformed pattern (missing ']')".into()));
                }
                let c = pat[q];
                q += 1;
                if c == b'%' {
                    if q >= pat.len() {
                        return Err(PatError("malformed pattern (ends with '%')".into()));
                    }
                    q += 1;
                }
                if pat.get(q) == Some(&b']') {
                    return Ok(q + 1);
                }
            }
        }
        Some(_) => Ok(p + 1),
    }
}

fn single_match(st: &State, s: usize, p: usize, ep: usize) -> bool {
    let Some(&c) = st.src.get(s) else {
        return false;
    };
    match st.pat[p] {
        b'.' => true,
        b'%' => class_match(c, st.pat[p + 1]),
        b'[' => match_bracket(c, st.pat, p, ep),
        pc => pc == c,
    }
}

fn capture_to_close(st: &State) -> Result<usize, PatError> {
    for i in (0..st.caps.len()).rev() {
        if st.caps[i].1 == CAP_UNFINISHED {
            return Ok(i);
        }
    }
    Err(PatError("invalid pattern capture".into()))
}

fn do_match(st: &mut State, mut s: usize, mut p: usize) -> Result<Option<usize>, PatError> {
    st.depth += 1;
    if st.depth > MAX_DEPTH {
        st.depth -= 1;
        return Err(PatError("pattern too complex".into()));
    }
    let r = do_match_inner(st, &mut s, &mut p);
    st.depth -= 1;
    r
}

fn do_match_inner(st: &mut State, s: &mut usize, p: &mut usize) -> Result<Option<usize>, PatError> {
    loop {
        if *p >= st.pat.len() {
            return Ok(Some(*s));
        }
        match st.pat[*p] {
            b'(' => {
                // position capture or start capture
                return if st.pat.get(*p + 1) == Some(&b')') {
                    if st.caps.len() >= MAX_CAPTURES {
                        return Err(PatError("too many captures".into()));
                    }
                    st.caps.push((*s, CAP_POSITION));
                    let r = do_match(st, *s, *p + 2)?;
                    if r.is_none() {
                        st.caps.pop();
                    }
                    Ok(r)
                } else {
                    if st.caps.len() >= MAX_CAPTURES {
                        return Err(PatError("too many captures".into()));
                    }
                    st.caps.push((*s, CAP_UNFINISHED));
                    let r = do_match(st, *s, *p + 1)?;
                    if r.is_none() {
                        st.caps.pop();
                    }
                    Ok(r)
                };
            }
            b')' => {
                let i = capture_to_close(st)?;
                st.caps[i].1 = (*s - st.caps[i].0) as isize;
                let r = do_match(st, *s, *p + 1)?;
                if r.is_none() {
                    st.caps[i].1 = CAP_UNFINISHED;
                }
                return Ok(r);
            }
            b'$' if *p + 1 == st.pat.len() => {
                return Ok(if *s == st.src.len() { Some(*s) } else { None });
            }
            b'%' => match st.pat.get(*p + 1) {
                Some(b'b') => {
                    // balanced match %bxy
                    let (Some(&x), Some(&y)) = (st.pat.get(*p + 2), st.pat.get(*p + 3)) else {
                        return Err(PatError(
                            "malformed pattern (missing arguments to '%b')".into(),
                        ));
                    };
                    if st.src.get(*s) != Some(&x) {
                        return Ok(None);
                    }
                    let mut bal = 1i32;
                    let mut q = *s + 1;
                    while q < st.src.len() {
                        if st.src[q] == y {
                            bal -= 1;
                            if bal == 0 {
                                return do_match(st, q + 1, *p + 4);
                            }
                        } else if st.src[q] == x {
                            bal += 1;
                        }
                        q += 1;
                    }
                    return Ok(None);
                }
                Some(b'f') => {
                    // frontier %f[set]
                    if st.pat.get(*p + 2) != Some(&b'[') {
                        return Err(PatError("missing '[' after '%f' in pattern".into()));
                    }
                    let ep = class_end(st, *p + 2)?;
                    let prev = if *s == 0 { 0u8 } else { st.src[*s - 1] };
                    let cur = st.src.get(*s).copied().unwrap_or(0);
                    if !match_bracket(prev, st.pat, *p + 2, ep)
                        && match_bracket(cur, st.pat, *p + 2, ep)
                    {
                        *p = ep;
                        continue;
                    }
                    return Ok(None);
                }
                Some(&d @ b'0'..=b'9') => {
                    // back-reference %1..%9 (%0 is invalid; guard before the
                    // `d - b'1'` subtraction so it cannot underflow)
                    if d == b'0' {
                        return Err(PatError("invalid capture index %0".into()));
                    }
                    let idx = (d - b'1') as usize;
                    if idx >= st.caps.len() || st.caps[idx].1 < 0 {
                        return Err(PatError(format!("invalid capture index %{}", (d - b'0'))));
                    }
                    let (cs, cl) = st.caps[idx];
                    let cl = cl as usize;
                    if st.src.len() - *s >= cl && st.src[cs..cs + cl] == st.src[*s..*s + cl] {
                        *s += cl;
                        *p += 2;
                        continue;
                    }
                    return Ok(None);
                }
                _ => { /* fall through to the default single-match path */ }
            },
            _ => {}
        }
        // default: single char class, possibly quantified
        let ep = class_end(st, *p)?;
        match st.pat.get(ep) {
            Some(b'?') => {
                if single_match(st, *s, *p, ep)
                    && let Some(r) = do_match(st, *s + 1, ep + 1)?
                {
                    return Ok(Some(r));
                }
                *p = ep + 1;
                continue;
            }
            Some(b'+') => {
                return if single_match(st, *s, *p, ep) {
                    max_expand(st, *s + 1, *p, ep)
                } else {
                    Ok(None)
                };
            }
            Some(b'*') => {
                return max_expand(st, *s, *p, ep);
            }
            Some(b'-') => {
                return min_expand(st, *s, *p, ep);
            }
            _ => {
                if single_match(st, *s, *p, ep) {
                    *s += 1;
                    *p = ep;
                    continue;
                }
                return Ok(None);
            }
        }
    }
}

fn max_expand(st: &mut State, s: usize, p: usize, ep: usize) -> Result<Option<usize>, PatError> {
    let mut i = 0;
    while single_match(st, s + i, p, ep) {
        i += 1;
    }
    loop {
        if let Some(r) = do_match(st, s + i, ep + 1)? {
            return Ok(Some(r));
        }
        if i == 0 {
            return Ok(None);
        }
        i -= 1;
    }
}

fn min_expand(
    st: &mut State,
    mut s: usize,
    p: usize,
    ep: usize,
) -> Result<Option<usize>, PatError> {
    loop {
        if let Some(r) = do_match(st, s, ep + 1)? {
            return Ok(Some(r));
        }
        if single_match(st, s, p, ep) {
            s += 1;
        } else {
            return Ok(None);
        }
    }
}

/// Whether the pattern contains any pattern-special character (gsub/find
/// fast path).
pub fn has_specials(pat: &[u8]) -> bool {
    pat.iter().any(|c| {
        matches!(
            c,
            b'^' | b'$' | b'*' | b'+' | b'?' | b'.' | b'(' | b')' | b'[' | b']' | b'%' | b'-'
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
