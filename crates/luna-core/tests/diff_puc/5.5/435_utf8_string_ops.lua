-- v2.15 P2.4 utf8: interaction with string library.
local s = "abc"
print(string.reverse(s))       -- byte reverse
print(string.upper(s))
print(#s)
-- string.sub is byte-based (not codepoint-based)
print(string.sub("café", 1, 3))    -- 3 bytes: "caf"
print(#string.sub("café", 1, 3))
