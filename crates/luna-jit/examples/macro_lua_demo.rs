//! MacroLua demo (v1.3 Phase ML) — opt-in compile-time macros.
//!
//! Run: `cargo run --example macro_lua_demo -p luna-jit`
//!
//! Pairs with `docs/compatibility.md` "MacroLua extensions" and
//! `docs/embedding.md` "MacroLua embedding".
//!
//! The MacroLua dialect adds an `@`-prefixed macro syntax on top of the
//! Lua 5.4 base. Macros run during parse (between lexing and AST build);
//! by the time the compiler / JIT see the program, all `@name(...)`
//! invocations are gone — replaced by the macro's expansion. Four
//! built-in macros are registered automatically on every
//! `Vm::new(LuaVersion::MacroLua)` Vm:
//!
//!   @quote{ body }                    — splice body verbatim
//!   @unquote(name)                    — splice a captured quote
//!   @if(cond, then-arm[, else-arm])   — compile-time conditional
//!   @gensym[(prefix)]                 — unique identifier
//!
//! This demo:
//!   1. Uses `@if` to pick one of two branches at parse time.
//!   2. Uses `@gensym` to mint a fresh local (hygiene).
//!   3. Registers a host-side `@double(x)` macro that rewrites
//!      `@double(x)` → `(x * 2)`.

use luna_core::frontend::error::SyntaxError;
use luna_core::frontend::macro_expander::{Macro, MacroCtx};
use luna_core::frontend::token::{Token, TokenInfo};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Host-side `@double(x)` — emits `(x * 2)`.
struct DoubleMacro;
impl Macro for DoubleMacro {
    fn expand(
        &self,
        args: &[Vec<TokenInfo>],
        ctx: &mut MacroCtx<'_>,
    ) -> Result<Vec<TokenInfo>, SyntaxError> {
        if args.len() != 1 {
            return Err(SyntaxError::new(
                ctx.line,
                format!("@double expects 1 arg, got {}", args.len()).into_bytes(),
            ));
        }
        let span = ctx.span;
        let line = ctx.line;
        let mk = |tok| TokenInfo { tok, span, line };
        let mut out = vec![mk(Token::LParen)];
        out.extend(args[0].clone());
        out.extend([mk(Token::Star), mk(Token::Int(2)), mk(Token::RParen)]);
        Ok(out)
    }
}

fn main() {
    // 1) @if — compile-time conditional. The "else" arm is *not even
    //    parsed at AST level* under MacroLua — the expander chose the
    //    `true` arm and the parser only sees `return 42`.
    {
        let mut vm = Vm::new(LuaVersion::MacroLua);
        let r = vm
            .eval("@if(true, @quote{ return 42 }, @quote{ return 0 })")
            .expect("eval @if");
        println!("1. @if(true, 42, 0) = {:?}", r);
    }

    // 2) @gensym — hygiene escape hatch. Each call yields a fresh
    //    identifier like `__lm_<counter>_<prefix>`, so macros that
    //    introduce locals can avoid shadowing user names.
    {
        let mut vm = Vm::new(LuaVersion::MacroLua);
        // `local @gensym = "alpha" local @gensym = "beta"` expands to
        // two distinct gensym'd locals; the second `return @gensym`
        // would mint a *third* (distinct) name, so reference the
        // second by capturing it ahead of time using a host macro
        // in real code. For demo purposes we just show that two
        // gensyms run without collision.
        vm.eval("local @gensym = 1 local @gensym = 2 return 0")
            .expect("eval @gensym");
        println!("2. @gensym minted two unique locals (no collision)");
    }

    // 3) Custom embedder macro — `@double(x)`.
    {
        let mut vm = Vm::new(LuaVersion::MacroLua);
        vm.define_macro("double", Box::new(DoubleMacro));
        let r = vm.eval("return @double(21)").expect("eval @double");
        println!("3. @double(21) = {:?}", r);

        // Macros compose under arg-position expansion (inside-out).
        let r2 = vm.eval("local y = 5; return @double(y)").expect("nested");
        println!("4. @double(y) where y = 5 → {:?}", r2);
    }

    // Notes:
    //   - PUC 5.1-5.5 sources continue to error on `@` exactly as
    //     before — MacroLua is purely additive.
    //   - Macro expansion happens once per `load()` call; there is no
    //     runtime overhead. The JIT never sees macros.
    //   - Hygiene is gensym-only in v1.3 (implicit quote-body scope
    //     rewrite is a future enhancement; see audit §5).
}
