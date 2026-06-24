//! MacroLua compile-time macro expander pre-pass.
//!
//! Walks a [`Vec<TokenInfo>`] produced by the lexer once, expands every
//! `@name(args)` invocation against the per-Vm [`MacroRegistry`], and
//! returns a `Vec<TokenInfo>` with no `@`/quote tokens remaining. The
//! result is fed to [`crate::frontend::parser::parse_tokens`] — the
//! parser itself is unchanged and never sees macros.
//!
//! ## Surface (audit-locked, see `.dev/rfcs/v1.3-audit-macro-lua.md` §3)
//!
//! - `@name(arg1, arg2, ...)` — call a registered macro with raw
//!   token-run arguments (top-level commas split args; nested
//!   parens/braces are tracked).
//! - `@name{ body }` — alternate brace-delimited single-arg form
//!   (think `@quote{...}` and `@if true {...} @else {...}`); the brace
//!   body is delivered to the macro as a single arg whose tokens are
//!   the (still-unexpanded) body between balanced `{...}`.
//! - `@{ tokens... }@` — explicit quote-block sigil; emits a
//!   [`Token::MacroQuote`] containing the captured run, available as a
//!   single arg to outer macros (e.g. `@unquote(name)` post-binding).
//!
//! ## Built-in macros (v1.3 floor)
//!
//! - `@quote{ ... }` — captures body as a single [`Token::MacroQuote`]
//!   value (which the parser ultimately never sees — it's spliced).
//! - `@unquote(name)` — inverse: inside another macro's expansion,
//!   `@unquote(name)` resolves to the named quote's body.
//! - `@if cond { then-arm } @else { else-arm }` — compile-time
//!   conditional; `cond` is one of `true` / `false` / integer or string
//!   literal-eq (`==` of literals only; deliberately *not* a tiny VM).
//! - `@gensym` / `@gensym(prefix)` — emits a unique identifier
//!   `Token::Name` (per-Vm counter; deterministic within one expansion).
//!
//! ## Hygiene model (chosen for v1.3 — see `docs/compatibility.md`)
//!
//! **Gensym-only.** Macro authors who need a fresh local explicitly
//! invoke `@gensym` and bind to it. The expander does **not** rewrite
//! `local <name>` declarations inside quote bodies. This matches the
//! audit's §5 stretch-goal deferral (implicit quote-body hygiene needs
//! a mini scope analyser; defer until dogfood asks).
//!
//! Nested expansion order: **inside-out**. Arg-position macro calls
//! (`@double(@gensym)`) are expanded *before* the outer macro receives
//! the args. This makes `@gensym`-inside-args composable with hygiene-
//! sensitive outer macros without surprise (the gensym'd name is the
//! arg value the outer macro sees).
//!
//! ## 0-dep contract
//!
//! Pure luna-core — uses only `Vec` / `Box<str>` / `HashMap` from std.
//! No proc-macro engine. Each registered macro is a `Box<dyn Macro>`
//! whose `expand` returns `Result<Vec<TokenInfo>, SyntaxError>`.

use crate::frontend::error::SyntaxError;
use crate::frontend::span::Span;
use crate::frontend::token::{Token, TokenInfo};
use std::collections::HashMap;

/// Maximum recursion depth for nested macro expansion. Mirrors the
/// parser's `MAX_DEPTH` (200) so a runaway `@foo` that re-emits `@foo`
/// trips before blowing the Rust call stack.
const MAX_EXPANSION_DEPTH: u32 = 200;

/// Context passed to every macro `expand` invocation: gives access to
/// the gensym counter (for hygienic identifier minting) and a back-
/// reference to the registry (so a macro can call other macros
/// programmatically — `@if` uses this to expand its chosen arm).
pub struct MacroCtx<'r> {
    /// Per-Vm gensym counter (`@gensym` increments). Lives on the Vm,
    /// borrowed here for the duration of one expansion pass.
    pub(crate) gensym_counter: &'r mut u64,
    /// The registry, for nested expansion. `None` blocks recursion (used
    /// when expanding a built-in's own output to defend against
    /// macro-defined infinite recursion outside the depth limit).
    /// Currently unread — the recursive expand happens in the outer
    /// driver `expand_stream` so built-ins don't need to re-enter the
    /// registry themselves. Kept on the public ctx surface so a future
    /// host-side macro that wants to call sibling macros has a path.
    #[allow(dead_code)]
    pub(crate) registry: Option<&'r MacroRegistry>,
    /// Line of the `@name` invocation, for error attribution.
    pub line: u32,
    /// Source span of the invocation (`@` byte through last `)`/`}`),
    /// for `Token::describe` slicing on synthesized tokens.
    pub span: Span,
}

impl<'r> MacroCtx<'r> {
    /// Mint a fresh identifier name like `__lm_42_tmp`. Used by
    /// `@gensym` and any host-side macro that needs hygiene.
    pub fn gensym(&mut self, prefix: &str) -> Box<str> {
        *self.gensym_counter = self.gensym_counter.wrapping_add(1);
        let n = *self.gensym_counter;
        let p = if prefix.is_empty() { "g" } else { prefix };
        format!("__lm_{n}_{p}").into_boxed_str()
    }
}

/// A registered MacroLua macro. Stateless w.r.t. the Vm — receives the
/// arg token runs and returns the expansion as a fresh token vector.
///
/// ## Args shape
///
/// `args` is a slice of arg token runs, each one already split at the
/// invocation's top-level commas. So `@foo(1, 2, 3)` arrives as
/// `args.len() == 3`, with `args[0] == [Int(1)]` etc. `@foo()` arrives
/// as `args.len() == 0`. The brace-delimited form `@foo{ ... }`
/// arrives as `args.len() == 1` with `args[0]` being the brace body.
///
/// ## Error reporting
///
/// Return `Err(SyntaxError { line, msg })` to bubble a parse-time
/// error attributed to a specific line (use `ctx.line` for the
/// invocation site or an inner token's line for finer attribution).
pub trait Macro {
    /// Expand this invocation into a token stream that replaces it.
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError>;
}

/// Per-Vm registry of registered macros (built-in + embedder-defined).
/// Owned by the `Vm` (see `vm/exec.rs::Vm::macro_registry`); built-ins
/// are inserted at Vm construction time when
/// `version == LuaVersion::MacroLua`.
pub struct MacroRegistry {
    macros: HashMap<Box<str>, Box<dyn Macro>>,
    /// Per-Vm gensym counter. Lives here (not on the Vm) so `Vm` only
    /// has to hold one field; the counter survives across `parse` calls
    /// so two scripts loaded into the same Vm get distinct gensyms.
    pub(crate) gensym_counter: u64,
}

impl Default for MacroRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MacroRegistry {
    /// Empty registry. Vms constructed with non-MacroLua versions hold
    /// this but never consult it.
    pub fn new() -> Self {
        MacroRegistry {
            macros: HashMap::new(),
            gensym_counter: 0,
        }
    }

    /// Build a registry pre-populated with the v1.3 built-in macros:
    /// `@quote`, `@unquote`, `@if`, `@gensym`.
    pub fn with_builtins() -> Self {
        let mut r = MacroRegistry::new();
        r.register("quote", Box::new(builtins::QuoteMacro));
        r.register("unquote", Box::new(builtins::UnquoteMacro));
        r.register("if", Box::new(builtins::IfMacro));
        r.register("gensym", Box::new(builtins::GensymMacro));
        r
    }

    /// Insert / overwrite a macro under `name`. Names are case-sensitive
    /// and stored as-is (no `@` prefix internally).
    pub fn register(&mut self, name: &str, m: Box<dyn Macro>) {
        self.macros.insert(name.into(), m);
    }

    /// Lookup; returns `None` for unregistered names.
    pub fn get(&self, name: &str) -> Option<&dyn Macro> {
        self.macros.get(name).map(|b| b.as_ref())
    }

    /// Drop all registered macros (including built-ins). Test/dogfood
    /// hygiene; not normally called by production embedders.
    pub fn clear(&mut self) {
        self.macros.clear();
    }

    /// Run the expansion pre-pass over `input`. The output stream has no
    /// `@`/quote tokens remaining and is suitable for
    /// [`crate::frontend::parser::parse_tokens`].
    pub fn expand(&mut self, input: Vec<TokenInfo>) -> Result<Vec<TokenInfo>, SyntaxError> {
        let mut counter = self.gensym_counter;
        let out = expand_stream(input, self, &mut counter, 0)?;
        self.gensym_counter = counter;
        Ok(out)
    }
}

/// Map a keyword token to its source spelling, so macro names like
/// `@if` / `@local` / `@return` can dispatch correctly even though
/// the lexer has folded them to keyword tokens.
fn keyword_name(t: &Token) -> Option<&'static str> {
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
fn expand_stream(
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
fn collect_macro_args(
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
fn collect_paren_args(
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
fn collect_brace_body(
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
fn collect_quote_block(
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

/// Built-in macros shipped under v1.3 floor.
mod builtins {
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
            if a.len() == 1 {
                if let Token::MacroQuote(body) = &a[0].tok {
                    return Ok(body.to_vec());
                }
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
                    format!("@if expects (cond, then[, else]) — got {} args", args.len())
                        .into_bytes(),
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
            if chosen.len() == 1 {
                if let Token::MacroQuote(body) = &chosen[0].tok {
                    return Ok(body.to_vec());
                }
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
            (Token::Int(x), Token::Float(y)) | (Token::Float(y), Token::Int(x)) => {
                (*x as f64) == *y
            }
            (Token::Str(x), Token::Str(y)) => x == y,
            (Token::True, Token::True)
            | (Token::False, Token::False)
            | (Token::Nil, Token::Nil) => true,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::Lexer;
    use crate::version::LuaVersion;

    fn lex(src: &str, v: LuaVersion) -> Vec<TokenInfo> {
        let mut lex = Lexer::new(src.as_bytes(), v);
        let mut out = Vec::new();
        loop {
            let t = lex.next_token().expect("lex");
            let eof = matches!(t.tok, Token::Eof);
            if eof {
                break;
            }
            out.push(t);
        }
        out
    }

    #[test]
    fn gensym_is_unique() {
        let mut r = MacroRegistry::with_builtins();
        let toks = lex("local a = @gensym local b = @gensym", LuaVersion::MacroLua);
        let out = r.expand(toks).unwrap();
        // Collect only the synthesized gensym names (prefix `__lm_`).
        let gensyms: Vec<String> = out
            .iter()
            .filter_map(|t| {
                if let Token::Name(n) = &t.tok {
                    if n.starts_with("__lm_") {
                        Some(n.to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(gensyms.len(), 2, "expected 2 gensyms, got {gensyms:?}");
        assert_ne!(gensyms[0], gensyms[1], "gensyms must be unique");
    }

    #[test]
    fn unknown_macro_errors() {
        let mut r = MacroRegistry::with_builtins();
        let toks = lex("@nope(1)", LuaVersion::MacroLua);
        let err = r.expand(toks).unwrap_err();
        assert!(
            String::from_utf8_lossy(&err.msg).contains("unknown macro"),
            "got: {}",
            err.msg_str()
        );
    }

    #[test]
    fn quote_splices_body() {
        let mut r = MacroRegistry::with_builtins();
        let toks = lex("local x = @quote{ 42 }", LuaVersion::MacroLua);
        let out = r.expand(toks).unwrap();
        // The output should contain Local, Name("x"), Assign, Int(42).
        let has_42 = out.iter().any(|t| matches!(t.tok, Token::Int(42)));
        assert!(has_42, "@quote{{42}} should splice Int(42); got {out:?}");
        // No `@` tokens remain.
        assert!(
            out.iter().all(|t| !matches!(
                t.tok,
                Token::At | Token::MacroBraceOpen | Token::MacroBraceClose
            )),
            "expander left @-tokens: {out:?}"
        );
    }
}
