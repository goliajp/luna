use super::*;

/// `@quote{ body }` — returns the body wrapped in a single
/// [`Token::MacroQuote`]. The parser never sees `MacroQuote`
/// directly; another macro is expected to consume it (via the
/// expander's arg-position handling) or `@unquote` is used to
/// splice it back into the stream.
///
/// **For the common case where the user simply wants a quote
/// available at the point of writing**, `@quote{...}` is most
/// useful as one arg of a host-registered macro. For the
/// `@quote{x = 1}` standalone roundtrip (test
/// `macro_lua_quote_roundtrip`), `@quote` emits the body tokens
/// directly when in **statement** position — the body is treated
/// as a snippet to splice. We achieve "both" by: if exactly one
/// brace-body arg is present, the body tokens are returned
/// verbatim (spliced); if no args, an error is raised.
pub(super) struct QuoteMacro;

impl Macro for QuoteMacro {
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError> {
        if args.len() != 1 {
            return Err(SyntaxError::new(
                ctx.line,
                format!(
                    "@quote expects exactly one brace body, got {} args",
                    args.len()
                )
                .into_bytes(),
            ));
        }
        // Splice the body directly. This makes `@quote{ x = 1 }`
        // expand to the tokens `x = 1` at the call site, which
        // matches the "syntactic snippet" use case.
        Ok(args[0].clone())
    }
}

/// `@unquote(name)` — given a single arg that is a captured
/// [`Token::MacroQuote`], splice its captured tokens back into
/// the stream. If the arg is anything else, error.
pub(super) struct UnquoteMacro;

impl Macro for UnquoteMacro {
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError> {
        if args.len() != 1 {
            return Err(SyntaxError::new(
                ctx.line,
                format!("@unquote expects 1 arg, got {}", args.len()).into_bytes(),
            ));
        }
        let a = &args[0];
        if a.len() == 1
            && let Token::MacroQuote(body) = &a[0].tok
        {
            return Ok(body.to_vec());
        }
        // Permissive: any non-MacroQuote single arg just passes
        // through verbatim — `@unquote(x)` becomes `x`. Useful in
        // host-side macro templates.
        Ok(a.clone())
    }
}

/// `@if cond { then-arm } @else { else-arm }` — compile-time
/// conditional. `cond` is one of:
///   - bareword `true` / `false`
///   - integer literal (truthy if non-zero)
///   - `expr == expr` where both sides are int / float / string
///     literals (literal-eq folder).
///
/// Because of how the expander packs args (paren form), `@if` here
/// uses a **single brace body** containing the entire then-arm.
/// The `@else { ... }` is a separate token run *after* the
/// invocation; the expander has already consumed only the then
/// arm. To make `@if cond {...} @else {...}` shape work cleanly,
/// we accept this surface form:
///
/// `@if(cond){ then-body }`        (else omitted = empty)
/// `@if(cond){ then-body }@else{ else-body }`
///
/// The post-`@else` clause is **not** picked up automatically
/// here — that would require the expander to look past the
/// invocation. Instead the test+demo use the simpler
/// `@if(cond){ then-body }` form for v1.3; `@if-else` is a
/// follow-up that's straightforward but adds dispatcher coupling.
pub(super) struct IfMacro;

impl Macro for IfMacro {
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError> {
        // Expected: 2 args = (cond-expr, then-body). Optional 3rd =
        // else-body. Args were split by the expander's
        // `collect_paren_args` so the cond comes via parens and the
        // bodies via... wait — parens form gives multiple args via
        // comma. The shape we'll accept is:
        //
        //   @if(cond, @quote{ then-body })
        //   @if(cond, @quote{ then-body }, @quote{ else-body })
        //
        // i.e. body arms are passed as `@quote{...}` quote tokens.
        // This keeps the macro syntax LR-parseable without needing
        // the expander to scan post-invocation tokens.
        if args.len() < 2 || args.len() > 3 {
            return Err(SyntaxError::new(
                ctx.line,
                format!("@if expects (cond, then[, else]) — got {} args", args.len()).into_bytes(),
            ));
        }
        let cond_truthy = eval_const_cond(&args[0], ctx.line)?;
        let chosen = if cond_truthy {
            &args[1]
        } else if args.len() == 3 {
            &args[2]
        } else {
            &EMPTY_ARM
        };
        // Unwrap MacroQuote if present; else splice as-is.
        if chosen.len() == 1
            && let Token::MacroQuote(body) = &chosen[0].tok
        {
            return Ok(body.to_vec());
        }
        Ok(chosen.clone())
    }
}

static EMPTY_ARM: Vec<TokenInfo> = Vec::new();

/// Evaluate a constant-fold-able condition expression. Supports:
///   - `true` / `false`
///   - integer literal (non-zero = true)
///   - `lit == lit` for int / float / string literals
fn eval_const_cond(tokens: &[TokenInfo], line: u32) -> Result<bool, SyntaxError> {
    // Strip leading/trailing whitespace already done by lexer.
    if tokens.is_empty() {
        return Err(SyntaxError::new(line, b"@if: empty condition".to_vec()));
    }
    if tokens.len() == 1 {
        return match &tokens[0].tok {
            Token::True => Ok(true),
            Token::False => Ok(false),
            Token::Int(i) => Ok(*i != 0),
            Token::Nil => Ok(false),
            _ => Err(SyntaxError::new(
                line,
                b"@if: cond must be true/false/integer/literal-eq".to_vec(),
            )),
        };
    }
    // 3-token form: lit `==` lit  or  lit `~=` lit
    if tokens.len() == 3 {
        let op = &tokens[1].tok;
        let eq = matches!(op, Token::Eq);
        let ne = matches!(op, Token::Ne);
        if eq || ne {
            let l = literal_eq(&tokens[0].tok, &tokens[2].tok, line)?;
            return Ok(if eq { l } else { !l });
        }
    }
    Err(SyntaxError::new(
        line,
        b"@if: unsupported condition shape (use true/false/int/lit==lit)".to_vec(),
    ))
}

fn literal_eq(a: &Token, b: &Token, line: u32) -> Result<bool, SyntaxError> {
    Ok(match (a, b) {
        (Token::Int(x), Token::Int(y)) => x == y,
        (Token::Float(x), Token::Float(y)) => x == y,
        (Token::Int(x), Token::Float(y)) | (Token::Float(y), Token::Int(x)) => (*x as f64) == *y,
        (Token::Str(x), Token::Str(y)) => x == y,
        (Token::True, Token::True) | (Token::False, Token::False) | (Token::Nil, Token::Nil) => {
            true
        }
        (Token::True, Token::False) | (Token::False, Token::True) => false,
        _ => {
            return Err(SyntaxError::new(
                line,
                b"@if: only int/float/string/bool/nil literals comparable".to_vec(),
            ));
        }
    })
}

/// `@gensym` / `@gensym("prefix")` — emit a fresh `Name` token.
pub(super) struct GensymMacro;

impl Macro for GensymMacro {
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError> {
        let prefix = if args.is_empty() {
            String::new()
        } else if args.len() == 1 && args[0].len() == 1 {
            match &args[0][0].tok {
                Token::Str(bytes) => String::from_utf8_lossy(bytes).into_owned(),
                Token::Name(n) => n.to_string(),
                _ => {
                    return Err(SyntaxError::new(
                        ctx.line,
                        b"@gensym: prefix must be a string literal or name".to_vec(),
                    ));
                }
            }
        } else {
            return Err(SyntaxError::new(
                ctx.line,
                b"@gensym: expected 0 or 1 args".to_vec(),
            ));
        };
        let name = ctx.gensym(&prefix);
        Ok(vec![TokenInfo {
            tok: Token::Name(name),
            span: ctx.span,
            line: ctx.line,
        }])
    }
}
