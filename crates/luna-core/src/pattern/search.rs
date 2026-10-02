//! Entry points: anchored matching, forward search and plain substring search.

use super::*;

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
