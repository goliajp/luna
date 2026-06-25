//! v2.0 Phase 5 CV gap fill — Lua-surface pattern-matching coverage.
//!
//! `crates/luna-core/src/pattern.rs` has a small inline `#[cfg(test)]`
//! module covering basics + sets/quantifiers + captures + a handful
//! of error cases. The audit-flagged gap is **integration-level
//! coverage through the `string.match` / `string.find` /
//! `string.gmatch` / `string.gsub` Lua APIs** — the path that
//! actually carries real-world traffic and that wires
//! `vm/lib_string.rs` into `pattern::find` / `pattern::match_at`.
//!
//! Each test here drives one pattern feature class through Lua user
//! syntax (single dialect = Lua 5.5 default; pattern engine is
//! dialect-agnostic). Cells cover:
//!
//! - Character classes (`%a` `%d` `%w` `%s` `%p` `%l` `%u`)
//! - Quantifiers (`*` `+` `?` `-`)
//! - Anchors (`^` `$`)
//! - Bracket sets (`[abc]` `[^abc]` `[a-z]`)
//! - Captures (multiple groups, position captures)
//! - Escapes (`%.` `%(` `%%`)
//! - `gmatch` iterator behavior
//! - `gsub` count return + replacement-arg variants
//! - `string.find` plain mode (init / plain=true)

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn vm() -> Vm {
    Vm::new(LuaVersion::Lua55)
}

fn eval_str(vm: &mut Vm, src: &str) -> String {
    let cl = vm.load(src.as_bytes(), b"=cv_pattern").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap_or(Value::Nil) {
        Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).to_string(),
        other => panic!("expected string, got {other:?}"),
    }
}

fn eval_int(vm: &mut Vm, src: &str) -> i64 {
    let cl = vm.load(src.as_bytes(), b"=cv_pattern").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap_or(Value::Nil) {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        other => panic!("expected int, got {other:?}"),
    }
}

/// Character-class shortcuts: `%a` (alpha), `%d` (digit), `%w`
/// (alnum), `%s` (space), `%l` (lower), `%u` (upper), `%p`
/// (punctuation). Each match the PUC-equivalent class.
#[test]
fn pattern_class_shortcuts_match_correctly() {
    let mut v = vm();
    assert_eq!(
        eval_str(&mut v, r#"return string.match("Hello", "%a+")"#),
        "Hello"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("abc123", "%d+")"#),
        "123"
    );
    // PUC `%w` = alphanumeric (no underscore); `_` ends the match.
    assert_eq!(
        eval_str(&mut v, r#"return string.match("abc_123", "%w+")"#),
        "abc"
    );
    assert_eq!(eval_str(&mut v, r#"return string.match("a b", "%s")"#), " ");
    assert_eq!(
        eval_str(&mut v, r#"return string.match("AbC", "%l+")"#),
        "b"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("AbC", "%u+")"#),
        "A"
    );
    assert_eq!(eval_str(&mut v, r#"return string.match("a,b", "%p")"#), ",");
}

/// Quantifiers: `*` (zero-or-more greedy), `+` (one-or-more greedy),
/// `?` (zero-or-one), `-` (zero-or-more lazy).
#[test]
fn pattern_quantifier_variants() {
    let mut v = vm();
    // `*` matches zero or more — "x*" against "ab" matches empty at pos 1
    assert_eq!(eval_int(&mut v, r#"return #string.match("ab", "x*")"#), 0);
    // `+` requires at least one
    assert_eq!(
        eval_str(&mut v, r#"return string.match("aaa", "a+")"#),
        "aaa"
    );
    // `?` matches optional 'b' in 'ab?c'
    assert_eq!(
        eval_str(&mut v, r#"return string.match("ac", "ab?c")"#),
        "ac"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("abc", "ab?c")"#),
        "abc"
    );
    // `-` lazy: <.-> matches "<a>" not "<a><b>"
    assert_eq!(
        eval_str(&mut v, r#"return string.match("<a><b>", "<.->")"#),
        "<a>"
    );
    // `*` greedy: <.*> matches the whole thing
    assert_eq!(
        eval_str(&mut v, r#"return string.match("<a><b>", "<.*>")"#),
        "<a><b>"
    );
}

/// Anchors: `^` (start), `$` (end). Both anchored = full match only.
#[test]
fn pattern_anchors_caret_dollar() {
    let mut v = vm();
    assert_eq!(
        eval_str(&mut v, r#"return string.match("hello", "^h")"#),
        "h"
    );
    // `^e` doesn't match "hello" (e isn't at start)
    assert_eq!(
        eval_str(&mut v, r#"return tostring(string.match("hello", "^e"))"#),
        "nil"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("hello", "o$")"#),
        "o"
    );
    // Full anchor: ^...$
    assert_eq!(
        eval_str(&mut v, r#"return string.match("abc", "^abc$")"#),
        "abc"
    );
    assert_eq!(
        eval_str(&mut v, r#"return tostring(string.match("abcd", "^abc$"))"#),
        "nil"
    );
}

/// Bracket sets: positive (`[abc]`), negative (`[^abc]`), range
/// (`[a-z]`), and combinations. The bracket walker in
/// `pattern.rs::match_bracket` is a load-bearing path with
/// nontrivial range/escape handling.
#[test]
fn pattern_bracket_sets() {
    let mut v = vm();
    assert_eq!(
        eval_str(&mut v, r#"return string.match("xyz123", "[0-9]+")"#),
        "123"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("abc", "[abc]+")"#),
        "abc"
    );
    // Negation: [^aeiou]+
    assert_eq!(
        eval_str(&mut v, r#"return string.match("hello", "[^aeiou]+")"#),
        "h"
    );
    // Range a-f
    assert_eq!(
        eval_str(&mut v, r#"return string.match("xyz_abc_xyz", "[a-f]+")"#),
        "abc"
    );
    // Combined: range + literal
    assert_eq!(
        eval_str(&mut v, r#"return string.match("12X34", "[0-9X]+")"#),
        "12X34"
    );
}

/// Multi-capture: `string.match` returns multiple values for
/// patterns with multiple `()` groups. Verifies capture-list
/// ordering + content split.
#[test]
fn pattern_multi_capture_groups() {
    let mut v = vm();
    let cl = v
        .load(
            br#"local a, b, c = string.match("abc123xyz", "(%a+)(%d+)(%a+)")
               return a .. "|" .. b .. "|" .. c"#,
            b"=cv_pattern",
        )
        .expect("load");
    let r = v.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"abc|123|xyz"),
        other => panic!("expected string, got {other:?}"),
    }
}

/// Position capture `()` returns the 1-based byte offset (NOT a
/// substring). Verifies the `Cap::Pos` path in `pattern.rs`.
#[test]
fn pattern_position_capture() {
    let mut v = vm();
    // "()b" → empty-position capture before the 'b'. In "abc",
    // that's position 2 (1-based).
    assert_eq!(
        eval_int(&mut v, r#"return (string.match("abc", "()b"))"#),
        2
    );
}

/// Escape sequences for pattern magic chars: `%.` matches literal
/// `.`, `%(` literal `(`, `%%` literal `%`.
#[test]
fn pattern_escape_special_chars() {
    let mut v = vm();
    assert_eq!(
        eval_str(&mut v, r#"return string.match("a.b", "a%.b")"#),
        "a.b"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("a(b)c", "a%(b%)c")"#),
        "a(b)c"
    );
    assert_eq!(
        eval_str(&mut v, r#"return string.match("100%", "%d+%%")"#),
        "100%"
    );
}

/// `gmatch` returns an iterator yielding successive matches. Used
/// with `for` to walk a delimited string.
#[test]
fn pattern_gmatch_iterator() {
    let mut v = vm();
    let cl = v
        .load(
            br#"local out = {}
               for w in string.gmatch("apple,banana,cherry", "[^,]+") do
                 out[#out+1] = w
               end
               return out[1] .. "|" .. out[2] .. "|" .. out[3] .. "|" .. #out"#,
            b"=cv_pattern",
        )
        .expect("load");
    let r = v.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"apple|banana|cherry|3"),
        other => panic!("expected string, got {other:?}"),
    }
}

/// `gmatch` with capture group yields the capture (not the full
/// match) per iteration.
#[test]
fn pattern_gmatch_yields_captures() {
    let mut v = vm();
    let cl = v
        .load(
            br#"local out = ""
               for k, v in string.gmatch("k1=v1;k2=v2", "(%w+)=(%w+)") do
                 out = out .. k .. ":" .. v .. ";"
               end
               return out"#,
            b"=cv_pattern",
        )
        .expect("load");
    let r = v.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"k1:v1;k2:v2;"),
        other => panic!("expected string, got {other:?}"),
    }
}

/// `string.find` in plain mode (4th arg = `true`) bypasses the
/// pattern engine and does a literal substring search. Exercises
/// the `pattern::plain_find` fast path.
#[test]
fn pattern_string_find_plain_mode() {
    let mut v = vm();
    // "%d+" treated as literal — won't match "abc123"
    assert_eq!(
        eval_str(
            &mut v,
            r#"return tostring(string.find("abc123", "%d+", 1, true))"#
        ),
        "nil"
    );
    // Same string treated as literal — DOES find "123" via plain
    assert_eq!(
        eval_int(&mut v, r#"return (string.find("abc123", "123", 1, true))"#),
        4
    );
    // Init argument: start search at position 5 (1-based)
    assert_eq!(
        eval_int(
            &mut v,
            r#"return (string.find("hello hello", "hello", 2, true))"#
        ),
        7
    );
}

/// `gsub` returns (result, count). Verifies BOTH return values.
#[test]
fn pattern_gsub_count_return() {
    let mut v = vm();
    let cl = v
        .load(
            br#"local s, n = string.gsub("a-b-c-d", "-", "/")
               return s .. ":" .. n"#,
            b"=cv_pattern",
        )
        .expect("load");
    let r = v.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"a/b/c/d:3"),
        other => panic!("expected string, got {other:?}"),
    }
}

/// `gsub` with max-replacement cap (5th arg).
#[test]
fn pattern_gsub_max_replacements() {
    let mut v = vm();
    let cl = v
        .load(
            br#"local s, n = string.gsub("a-b-c-d-e", "-", "/", 2)
               return s .. ":" .. n"#,
            b"=cv_pattern",
        )
        .expect("load");
    let r = v.call_value(Value::Closure(cl), &[]).expect("eval");
    match r.into_iter().next().unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"a/b/c-d-e:2"),
        other => panic!("expected string, got {other:?}"),
    }
}
