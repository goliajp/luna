use super::*;

fn m(src: &str, pat: &str) -> Option<(usize, usize)> {
    find(src.as_bytes(), pat.as_bytes(), 0)
        .unwrap()
        .map(|m| (m.start, m.end))
}

#[test]
fn basics() {
    assert_eq!(m("hello", "l+"), Some((2, 4)));
    assert_eq!(m("hello", "^h"), Some((0, 1)));
    assert_eq!(m("hello", "^e"), None);
    assert_eq!(m("hello", "o$"), Some((4, 5)));
    assert_eq!(m("hello", "%a+"), Some((0, 5)));
    assert_eq!(m("a1b2", "%d"), Some((1, 2)));
    assert_eq!(m("abc", "a.c"), Some((0, 3)));
    assert_eq!(m("", ".*"), Some((0, 0)));
    assert_eq!(m("abc", "x*"), Some((0, 0)));
}

#[test]
fn sets_and_quantifiers() {
    assert_eq!(m("hello world", "[aeiou]"), Some((1, 2)));
    assert_eq!(m("hello", "[^aeiou]+"), Some((0, 1)));
    assert_eq!(m("x123y", "[0-9]+"), Some((1, 4)));
    assert_eq!(m("aaa", "a-"), Some((0, 0)));
    assert_eq!(m("<a><b>", "<.->"), Some((0, 3)));
    assert_eq!(m("<a><b>", "<.*>"), Some((0, 6)));
    assert_eq!(m("abc", "ab?c"), Some((0, 3)));
    assert_eq!(m("ac", "ab?c"), Some((0, 2)));
}

#[test]
fn captures_and_specials() {
    let mm = find(b"key=value", b"(%w+)=(%w+)", 0).unwrap().unwrap();
    assert_eq!(mm.caps.len(), 2);
    assert_eq!(mm.caps[0], Cap::Span(0, 3));
    assert_eq!(mm.caps[1], Cap::Span(4, 9));
    // position capture
    let mm = find(b"abc", b"a()b", 0).unwrap().unwrap();
    assert_eq!(mm.caps[0], Cap::Pos(1));
    // balanced
    assert_eq!(m("(foo(bar))baz", "%b()"), Some((0, 10)));
    // frontier
    assert_eq!(m("THE (quick) fox", "%f[%a]%a+"), Some((0, 3)));
    // back-reference
    assert_eq!(m("abcabc", "(abc)%1"), Some((0, 6)));
    assert_eq!(m("abcabd", "(abc)%1"), None);
}

#[test]
fn errors() {
    assert!(find(b"x", b"%", 0).is_err());
    assert!(find(b"x", b"[abc", 0).is_err());
    assert!(find(b"a", b"(a", 0).is_err()); // unfinished capture
    assert!(find(b"x", b"%1", 0).is_err());
}
