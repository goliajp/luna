use super::*;

fn toks(src: &str, v: LuaVersion) -> Result<Vec<Token>, SyntaxError> {
    let mut lex = Lexer::new(src.as_bytes(), v);
    let mut out = Vec::new();
    loop {
        let t = lex.next_token()?;
        if t.tok == Token::Eof {
            return Ok(out);
        }
        out.push(t.tok);
    }
}

#[test]
fn numbers_55() {
    let v = LuaVersion::Lua55;
    assert_eq!(toks("3", v).unwrap(), vec![Token::Int(3)]);
    assert_eq!(toks("3.0", v).unwrap(), vec![Token::Float(3.0)]);
    assert_eq!(toks("345", v).unwrap(), vec![Token::Int(345)]);
    assert_eq!(toks("0xff", v).unwrap(), vec![Token::Int(255)]);
    assert_eq!(toks("0x1p4", v).unwrap(), vec![Token::Float(16.0)]);
    assert_eq!(toks("0x0.8", v).unwrap(), vec![Token::Float(0.5)]);
    assert_eq!(toks("0xA.8p1", v).unwrap(), vec![Token::Float(21.0)]);
    assert_eq!(toks(".5e2", v).unwrap(), vec![Token::Float(50.0)]);
    assert_eq!(toks("1e2", v).unwrap(), vec![Token::Float(100.0)]);
    // decimal i64 overflow becomes a float
    assert_eq!(
        toks("9223372036854775808", v).unwrap(),
        vec![Token::Float(9223372036854775808.0)]
    );
    // hex wraps modulo 2^64
    assert_eq!(toks("0xFFFFFFFFFFFFFFFF", v).unwrap(), vec![Token::Int(-1)]);
    assert!(toks("3..2", v).is_err());
    assert!(toks("3a", v).is_err());
    assert!(toks("0x", v).is_err());
    assert!(toks("1e+", v).is_err());
}

#[test]
fn numbers_51() {
    let v = LuaVersion::Lua51;
    assert_eq!(toks("3", v).unwrap(), vec![Token::Float(3.0)]);
    assert_eq!(toks("0x10", v).unwrap(), vec![Token::Float(16.0)]);
    // PUC 5.1 converts numerals with C99 `strtod`, which reads hex floats
    assert_eq!(toks("0x1p4", v).unwrap(), vec![Token::Float(16.0)]);
}

#[test]
fn strings() {
    let v = LuaVersion::Lua55;
    assert_eq!(
        toks(r#""a\65\x42\u{48}c""#, v).unwrap(),
        vec![Token::Str(b"aABHc".to_vec())]
    );
    assert_eq!(
        toks("\"a\\z  \n  b\"", v).unwrap(),
        vec![Token::Str(b"ab".to_vec())]
    );
    assert_eq!(
        toks("[==[\nhey]]==]", v).unwrap(),
        vec![Token::Str(b"hey]".to_vec())]
    );
    assert!(toks(r#""\x4""#, v).is_err());
    assert!(toks(r#""\300""#, v).is_err());
    // 5.1 has no `\x`: an unknown escape is the character itself
    assert_eq!(
        toks(r#""\x41""#, LuaVersion::Lua51).unwrap(),
        vec![Token::Str(b"x41".to_vec())]
    );
}

#[test]
fn version_gates() {
    assert!(
        toks("a // b", LuaVersion::Lua51).is_err() || {
            // `//` lexes as two Slash tokens in 5.1; parser rejects later
            toks("a // b", LuaVersion::Lua51)
                .unwrap()
                .contains(&Token::Slash)
        }
    );
    assert_eq!(
        toks("goto", LuaVersion::Lua51).unwrap(),
        vec![Token::Name("goto".into())]
    );
    assert_eq!(toks("goto", LuaVersion::Lua55).unwrap(), vec![Token::Goto]);
    // `global` is a contextual keyword (parser decides); the lexer always
    // produces a plain name in every version.
    assert_eq!(
        toks("global", LuaVersion::Lua54).unwrap(),
        vec![Token::Name("global".into())]
    );
    assert_eq!(
        toks("global", LuaVersion::Lua55).unwrap(),
        vec![Token::Name("global".into())]
    );
    assert!(toks("a & b", LuaVersion::Lua51).is_err());
}

#[test]
fn shebang_and_bom() {
    // shebang/BOM stripping is a file-load concern, not the lexer's: the
    // helper removes them, leaving the newline so line counts are kept.
    assert_eq!(
        Lexer::strip_shebang_bom(b"#!/usr/bin/lua\nreturn"),
        b"\nreturn"
    );
    assert_eq!(Lexer::strip_shebang_bom(&[0xEF, 0xBB, 0xBF, b'x']), b"x");
    // a string chunk keeps `#` as the length operator (no stripping here)
    let v = LuaVersion::Lua55;
    assert_eq!(
        toks("#a", v).unwrap(),
        vec![Token::Hash, Token::Name("a".into())]
    );
}
