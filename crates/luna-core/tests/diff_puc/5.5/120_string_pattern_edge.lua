-- v2.12 CORPUS-III: string patterns edge cases.
-- anchors
print(string.match("hello", "^h"))
print(string.match("hello", "o$"))
print(string.match("hello", "^hello$"))
-- character classes
print(string.match("A1b2", "%a%d%a%d"))
print(string.match("abc", "%A"))     -- nil
print(string.match("a b", "%s"))
print(string.match("abc", "[aeiou]"))
-- +/* /?
print(string.match("aabbcc", "a+"))
print(string.match("bbcc", "a*"))    -- empty match
print(string.match("bb", "a?"))
-- balanced
print(string.match("f(x, y(z))g", "%b()"))
