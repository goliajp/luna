//! Chunk names as PUC renders them into `short_src`.

use crate::version::LuaVersion;

/// PUC `luaO_chunkid`: render a chunk's `source` into its `short_src` form
/// for a `LUA_IDSIZE` (60) buffer. `=name` keeps the literal
/// (head-truncated), `@file` keeps the tail of the path behind `...`, and
/// anything else is a string shown as `[string "first line..."]`. 5.1
/// reserves room for its message decorations and so keeps less of a long
/// path (52 bytes, 5.2+: 56) or string (43 bytes, 5.2+: 45), and also
/// stops a string at a carriage return.
pub(crate) fn chunk_id(v: LuaVersion, source: &[u8]) -> Vec<u8> {
    chunk_id_in(v, source, 60)
}

/// The chunk name a syntax error is prefixed with: 5.1's lexer renders it
/// into an 80-byte buffer (`MAXSRC`), later versions use `LUA_IDSIZE`.
pub(crate) fn syntax_chunk_id(v: LuaVersion, source: &[u8]) -> Vec<u8> {
    let idsize = if v == LuaVersion::Lua51 { 80 } else { 60 };
    chunk_id_in(v, source, idsize)
}

fn chunk_id_in(v: LuaVersion, source: &[u8], idsize: usize) -> Vec<u8> {
    const RETS: &[u8] = b"...";
    const PRE: &[u8] = b"[string \"";
    const POS: &[u8] = b"\"]";
    let v51 = v == LuaVersion::Lua51;
    let mut out = Vec::new();
    match source.first() {
        Some(b'=') => {
            // at most idsize - 1 bytes of the name, the rest being the NUL
            let s = &source[1..];
            out.extend_from_slice(&s[..s.len().min(idsize - 1)]);
        }
        Some(b'@') => {
            let s = &source[1..];
            // 5.1: `bufflen -= sizeof(" '...' ")`; 5.2+: the tail that fits
            // after "..." with the NUL
            let keep = if v51 {
                idsize - 8
            } else {
                idsize - RETS.len() - 1
            };
            let fits = if v51 {
                s.len() <= keep
            } else {
                source.len() <= idsize
            };
            if fits {
                out.extend_from_slice(s);
            } else {
                out.extend_from_slice(RETS);
                out.extend_from_slice(&s[s.len() - keep..]);
            }
        }
        _ => {
            out.extend_from_slice(PRE);
            if v51 {
                // `bufflen -= sizeof(" [string \"...\"] ")`
                let bufflen = idsize - 17;
                let line = source
                    .iter()
                    .position(|&c| c == b'\n' || c == b'\r')
                    .unwrap_or(source.len());
                let len = line.min(bufflen);
                out.extend_from_slice(&source[..len]);
                if len < source.len() {
                    out.extend_from_slice(RETS);
                }
            } else {
                let nl = source.iter().position(|&c| c == b'\n');
                let bufflen = idsize - PRE.len() - RETS.len() - POS.len() - 1;
                if source.len() < bufflen && nl.is_none() {
                    out.extend_from_slice(source);
                } else {
                    let len = nl.unwrap_or(source.len()).min(bufflen);
                    out.extend_from_slice(&source[..len]);
                    out.extend_from_slice(RETS);
                }
            }
            out.extend_from_slice(POS);
        }
    }
    out
}
