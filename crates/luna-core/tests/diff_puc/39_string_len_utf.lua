-- v2.10 CORPUS: string byte-length semantics.
print(#"")         -- 0
print(#"a")        -- 1
print(#"café")     -- 5 (byte-len, not char-len)
print(string.len("hello"))
print(string.len(""))
-- string.len == # for strings
print(#"hello" == string.len("hello"))
