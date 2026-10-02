//! MacroLua compile-time macro expander pre-pass.
//!
//! Walks a [`Vec<TokenInfo>`] produced by the lexer once, expands every
//! `@name(args)` invocation against the per-Vm [`MacroRegistry`], and
//! returns a `Vec<TokenInfo>` with no `@`/quote tokens remaining. The
//! result is fed to [`crate::frontend::parser::parse_tokens`] — the
//! parser itself is unchanged and never sees macros.
//!
//! ## Surface (audit-locked
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

mod expand;
use expand::expand_stream;

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

/// Built-in macros shipped under v1.3 floor.
mod builtins;

#[cfg(test)]
mod tests;
