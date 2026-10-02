//! The expansion loop and the argument collectors it uses.

use super::*;

/// Map a keyword token to its source spelling, so macro names like
/// `@if` / `@local` / `@return` can dispatch correctly even though
/// the lexer has folded them to keyword tokens.
pub(super) fn keyword_name(t: &Token) -> Option<&'static str> {
    Some(match t {
        Token::And => "and",
        Token::Break => "break",
        Token::Do => "do",
        Token::Else => "else",
        Token::Elseif => "elseif",
        Token::End => "end",
        Token::False => "false",
        Token::For => "for",
        Token::Function => "function",
        Token::Global => "global",
        Token::Goto => "goto",
        Token::If => "if",
        Token::In => "in",
        Token::Local => "local",
        Token::Nil => "nil",
        Token::Not => "not",
        Token::Or => "or",
        Token::Repeat => "repeat",
        Token::Return => "return",
        Token::Then => "then",
        Token::True => "true",
        Token::Until => "until",
        Token::While => "while",
        _ => return None,
    })
}

/// Core expansion loop. Recursive (depth-checked) so an arg-position
/// macro call (`@double(@gensym)`) is expanded inside-out before its
/// enclosing macro sees the result.
pub(super) fn expand_stream(
    input: Vec<TokenInfo>,
    registry: &MacroRegistry,
    gensym_counter: &mut u64,
    depth: u32,
) -> Result<Vec<TokenInfo>, SyntaxError> {
    if depth > MAX_EXPANSION_DEPTH {
        let line = input.first().map(|t| t.line).unwrap_or(1);
        return Err(SyntaxError::new(
            line,
            b"macro expansion depth exceeded (200) near '@'".to_vec(),
        ));
    }

    let mut out: Vec<TokenInfo> = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        match &input[i].tok {
            Token::At => {
                let inv_line = input[i].line;
                let inv_start = input[i].span;
                // expect Token::Name or a keyword-token immediately after
                // `@` (the macro namespace overlaps Lua keywords, e.g.
                // `@if` / `@local` / `@return` are useful spellings).
                let name_idx = i + 1;
                let name = match input.get(name_idx).map(|t| &t.tok) {
                    Some(Token::Name(n)) => n.clone(),
                    Some(other) => {
                        if let Some(kw) = keyword_name(other) {
                            kw.into()
                        } else {
                            return Err(SyntaxError::new(
                                inv_line,
                                b"macro name expected after '@'".to_vec(),
                            ));
                        }
                    }
                    None => {
                        return Err(SyntaxError::new(
                            inv_line,
                            b"macro name expected after '@'".to_vec(),
                        ));
                    }
                };
                // Parse arg block: either `(args)`, `{ body }`, or empty.
                let mut cursor = name_idx + 1;
                let (raw_args, after) = collect_macro_args(&input, cursor, inv_line)?;
                cursor = after;

                // Recursively expand each arg run (inside-out hygiene
                // model — see module docs).
                let mut expanded_args: Vec<Vec<TokenInfo>> = Vec::with_capacity(raw_args.len());
                for a in raw_args {
                    expanded_args.push(expand_stream(a, registry, gensym_counter, depth + 1)?);
                }

                // Dispatch to the registry.
                let macro_impl = registry.get(&name).ok_or_else(|| {
                    SyntaxError::new(inv_line, format!("unknown macro '@{name}'").into_bytes())
                })?;

                // Span of the entire invocation, from `@` to the byte
                // after the last arg-block token (best-effort; used for
                // error reporting on synthesized tokens).
                let end_span = if cursor > 0 && cursor <= input.len() {
                    input[cursor - 1].span
                } else {
                    inv_start
                };
                let full_span = Span::new(inv_start.start as usize, end_span.end as usize);

                let mut ctx = MacroCtx {
                    gensym_counter,
                    registry: Some(registry),
                    line: inv_line,
                    span: full_span,
                };
                let mut expanded = macro_impl.expand(&expanded_args, &mut ctx)?;
                // Recursively expand the macro's output as well (so a
                // macro can produce `@foo(...)` calls of other macros).
                // Depth +1 guards against runaway recursion.
                expanded = expand_stream(expanded, registry, gensym_counter, depth + 1)?;
                out.extend(expanded);
                i = cursor;
            }
            Token::MacroBraceOpen => {
                // Bare `@{ ... }@` block at statement / arg position —
                // captures as a MacroQuote token in `out`. Useful when
                // the body is later consumed via `@unquote` of a bound
                // name (host-registered macro pattern).
                let block_line = input[i].line;
                let (body, after) = collect_quote_block(&input, i, block_line)?;
                let span = Span::new(
                    input[i].span.start as usize,
                    input[after - 1].span.end as usize,
                );
                // Recursively expand the body so it's macro-free when
                // un-quoted.
                let body_expanded = expand_stream(body, registry, gensym_counter, depth + 1)?;
                out.push(TokenInfo {
                    tok: Token::MacroQuote(body_expanded.into_boxed_slice()),
                    span,
                    line: block_line,
                });
                i = after;
            }
            Token::MacroBraceClose => {
                return Err(SyntaxError::new(
                    input[i].line,
                    b"unexpected '}@' (no matching '@{')".to_vec(),
                ));
            }
            Token::MacroQuote(_) => {
                // Synthetic — pass through. (The parser never sees it
                // because @unquote / built-ins splice it away first; if
                // one survives to here it's user error and we surface
                // it as a syntax error.)
                return Err(SyntaxError::new(
                    input[i].line,
                    b"stray macro-quote token left in stream (forgot '@unquote'?)".to_vec(),
                ));
            }
            _ => {
                out.push(input[i].clone());
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Parse the arg block immediately following `@name`: `(a, b)`, `{ ... }`,
/// or empty. Returns the raw arg runs (un-expanded) and the cursor
/// position just after the last consumed token.
pub(super) fn collect_macro_args(
    input: &[TokenInfo],
    start: usize,
    inv_line: u32,
) -> Result<(Vec<Vec<TokenInfo>>, usize), SyntaxError> {
    if start >= input.len() {
        return Ok((Vec::new(), start));
    }
    match &input[start].tok {
        Token::LParen => collect_paren_args(input, start, inv_line),
        Token::LBrace => {
            // `@name{ body }` — single brace-body arg.
            let (body, after) = collect_brace_body(input, start, inv_line)?;
            Ok((vec![body], after))
        }
        Token::MacroBraceOpen => {
            // `@name@{ body }@` — explicit quote-block as single arg.
            let (body, after) = collect_quote_block(input, start, inv_line)?;
            Ok((vec![body], after))
        }
        _ => {
            // No arg block — `@gensym`, etc.
            Ok((Vec::new(), start))
        }
    }
}

/// `(a, b, c)` — splits at top-level commas; nested parens / brackets /
/// braces / quote blocks are tracked.
pub(super) fn collect_paren_args(
    input: &[TokenInfo],
    lparen_idx: usize,
    inv_line: u32,
) -> Result<(Vec<Vec<TokenInfo>>, usize), SyntaxError> {
    debug_assert!(matches!(input[lparen_idx].tok, Token::LParen));
    let mut depth_paren = 1u32;
    let mut depth_brace = 0u32;
    let mut depth_bracket = 0u32;
    let mut depth_quote = 0u32;
    let mut args: Vec<Vec<TokenInfo>> = Vec::new();
    let mut cur: Vec<TokenInfo> = Vec::new();
    let mut i = lparen_idx + 1;
    while i < input.len() {
        match &input[i].tok {
            Token::LParen => {
                depth_paren += 1;
                cur.push(input[i].clone());
            }
            Token::RParen => {
                depth_paren -= 1;
                if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 && depth_quote == 0 {
                    if !cur.is_empty() || !args.is_empty() {
                        args.push(std::mem::take(&mut cur));
                    }
                    return Ok((args, i + 1));
                }
                cur.push(input[i].clone());
            }
            Token::LBrace => {
                depth_brace += 1;
                cur.push(input[i].clone());
            }
            Token::RBrace => {
                if depth_brace == 0 {
                    return Err(SyntaxError::new(
                        input[i].line,
                        b"unexpected '}' inside macro arg list".to_vec(),
                    ));
                }
                depth_brace -= 1;
                cur.push(input[i].clone());
            }
            Token::LBracket => {
                depth_bracket += 1;
                cur.push(input[i].clone());
            }
            Token::RBracket => {
                depth_bracket = depth_bracket.saturating_sub(1);
                cur.push(input[i].clone());
            }
            Token::MacroBraceOpen => {
                depth_quote += 1;
                cur.push(input[i].clone());
            }
            Token::MacroBraceClose => {
                if depth_quote == 0 {
                    return Err(SyntaxError::new(
                        input[i].line,
                        b"unexpected '}@' inside macro arg list".to_vec(),
                    ));
                }
                depth_quote -= 1;
                cur.push(input[i].clone());
            }
            Token::Comma
                if depth_paren == 1
                    && depth_brace == 0
                    && depth_bracket == 0
                    && depth_quote == 0 =>
            {
                args.push(std::mem::take(&mut cur));
            }
            _ => cur.push(input[i].clone()),
        }
        i += 1;
    }
    Err(SyntaxError::new(
        inv_line,
        b"unterminated macro arg list (missing ')')".to_vec(),
    ))
}

/// `{ ... }` brace-body — captures everything between balanced braces
/// as a single token run. Nested braces are passed through.
pub(super) fn collect_brace_body(
    input: &[TokenInfo],
    lbrace_idx: usize,
    inv_line: u32,
) -> Result<(Vec<TokenInfo>, usize), SyntaxError> {
    debug_assert!(matches!(input[lbrace_idx].tok, Token::LBrace));
    let mut depth = 1u32;
    let mut body: Vec<TokenInfo> = Vec::new();
    let mut i = lbrace_idx + 1;
    while i < input.len() {
        match &input[i].tok {
            Token::LBrace => {
                depth += 1;
                body.push(input[i].clone());
            }
            Token::RBrace => {
                depth -= 1;
                if depth == 0 {
                    return Ok((body, i + 1));
                }
                body.push(input[i].clone());
            }
            _ => body.push(input[i].clone()),
        }
        i += 1;
    }
    Err(SyntaxError::new(
        inv_line,
        b"unterminated macro brace body (missing '}')".to_vec(),
    ))
}

/// `@{ tokens... }@` — captures everything between balanced
/// `@{`/`}@` sigils. Nested `@{...}@` is supported.
pub(super) fn collect_quote_block(
    input: &[TokenInfo],
    open_idx: usize,
    inv_line: u32,
) -> Result<(Vec<TokenInfo>, usize), SyntaxError> {
    debug_assert!(matches!(input[open_idx].tok, Token::MacroBraceOpen));
    let mut depth = 1u32;
    let mut body: Vec<TokenInfo> = Vec::new();
    let mut i = open_idx + 1;
    while i < input.len() {
        match &input[i].tok {
            Token::MacroBraceOpen => {
                depth += 1;
                body.push(input[i].clone());
            }
            Token::MacroBraceClose => {
                depth -= 1;
                if depth == 0 {
                    return Ok((body, i + 1));
                }
                body.push(input[i].clone());
            }
            _ => body.push(input[i].clone()),
        }
        i += 1;
    }
    Err(SyntaxError::new(
        inv_line,
        b"unterminated quote block (missing '}@')".to_vec(),
    ))
}
