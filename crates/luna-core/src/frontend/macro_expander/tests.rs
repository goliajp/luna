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
