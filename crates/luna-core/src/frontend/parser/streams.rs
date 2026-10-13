//! Parses over a source read piece by piece and over a pre-expanded token
//! stream.

use super::*;

/// [`parse_reusing`] over a source read piece by piece: `first` is the
/// piece already read, and `feed` appends the next one or returns false at
/// the end. The parser asks for a piece only when the scan moves past the
/// bytes it has, as PUC's does, so it stops reading at a syntax error.
pub(crate) fn parse_stream<'f>(
    first: LVec<u8>,
    feed: &'f mut crate::frontend::lexer::Feed<'f>,
    version: LuaVersion,
    c_depth: u32,
    mut scratch: ParseScratch,
) -> Result<Parsed, (SyntaxError, Option<(Failed, LVec<u8>)>)> {
    scratch.clear();
    let names = scratch.chunk.names.take();
    let len = first.len();
    let lex_buf = scratch.lex_buf.take();
    let lex = Lexer::interning_stream(Stream::new(first, feed), version, names, lex_buf);
    let r = parse_from_source(
        TokenSource::Stream(lex),
        version,
        c_depth,
        len,
        &mut scratch,
    );
    match r {
        Ok(()) => Ok(scratch.into_parsed()),
        // with the source read, to find an error it meets first
        Err(e) => Err((e, scratch.failed.zip(scratch.streamed.take()))),
    }
}

/// Parse a **pre-materialized** token stream. Used by the MacroLua
/// expander pre-pass — it walks the lexer output once, expands
/// `@name(...)` invocations against the per-Vm macro registry, and
/// feeds the resulting `Vec<TokenInfo>` here.
pub fn parse_tokens(
    tokens: Vec<TokenInfo>,
    src: &[u8],
    version: LuaVersion,
) -> Result<Chunk, SyntaxError> {
    let mem = MemOwner::system();
    parse_tokens_at_depth(tokens, src, version, 0, mem.clone()).map(|p| p.chunk)
}

/// [`parse_tokens`] at a C depth (see [`parse_at_depth`]), building the
/// tree in vectors from `mem`.
pub(crate) fn parse_tokens_at_depth(
    tokens: Vec<TokenInfo>,
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    mem: MemOwner,
) -> Result<Parsed, SyntaxError> {
    let names = Names::with_capacity(mem.mem(), src.len());
    let mut scratch = ParseScratch::new(mem);
    parse_from_source(
        TokenSource::PreExpanded {
            tokens,
            cursor: 0,
            src,
            names,
        },
        version,
        c_depth,
        src.len(),
        &mut scratch,
    )?;
    Ok(scratch.into_parsed())
}
