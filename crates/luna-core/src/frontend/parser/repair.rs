//! The source a failed parse read before the token it failed on, completed
//! so that it parses.
//!
//! PUC generates code while it parses, so a limit its code generator meets
//! before its parser meets a syntax error is the error the load reports.
//! luna compiles a finished tree: to find such an error, the source before
//! the failing token is completed, one expected token at a time, until it
//! parses, and compiled. The tokens added come after the failing one, so
//! an error the compiler raises on them is not one PUC meets first.

use super::*;

/// Tokens added before giving up: about one per construct left open.
const MAX_ADDED: usize = 400;

/// What a parse of `src` that failed at `failed` read, completed so that it
/// parses, or `None` when the error is not one a missing token explains.
pub(crate) fn completed(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    failed: Failed,
) -> Option<Vec<u8>> {
    if failed.first {
        return Some(Vec::new());
    }
    let at = token_at(src, version, c_depth, failed.at)?;
    // a token that failed to scan ends the source after the one before it
    let cut = if failed.scanning {
        at.span.end
    } else {
        at.span.start
    };
    let mut out = src[..cut as usize].to_vec();
    for _ in 0..MAX_ADDED {
        let mem = MemOwner::system();
        let mut scratch = ParseScratch::new(mem.clone());
        let Err(e) = parse_in_place(&out, version, c_depth, &mut scratch) else {
            return Some(out);
        };
        let f = scratch.failed?;
        if f.scanning || f.at < failed.at {
            return None;
        }
        out.push(b' ');
        out.extend_from_slice(missing(&e.msg)?);
    }
    None
}

/// The token a parse error at the end of a source asks for.
fn missing(msg: &[u8]) -> Option<&'static [u8]> {
    let starts = |p: &str| msg.starts_with(p.as_bytes());
    if starts("<name> expected") || starts("'<name>' expected") {
        return Some(b"_");
    }
    if starts("unexpected symbol") {
        return Some(b"nil");
    }
    // 5.2+: an expression statement that is not a call
    if starts("syntax error") || starts("'=' or 'in' expected") {
        return Some(b"=");
    }
    if starts("function arguments expected") {
        return Some(b"()");
    }
    let rest = msg.strip_prefix(b"'")?;
    let end = rest.iter().position(|&b| b == b'\'')?;
    if !rest[end..].starts_with(b"' expected") {
        return None;
    }
    const TOKENS: [&[u8]; 13] = [
        b"end", b"=", b")", b"]", b"}", b"(", b",", b"do", b"then", b"until", b"in", b"::", b">",
    ];
    TOKENS.into_iter().find(|t| *t == &rest[..end])
}
