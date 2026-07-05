-- v2.12 CORPUS-III: greedy vs lazy patterns.
print(string.match("aaaa", "a*"))    -- aaaa (greedy)
print(string.match("aaaa", "a-"))    -- empty (lazy)
-- lazy captures shortest
print(string.match("<b>text</b>", "<(.-)>"))
-- greedy captures longest
print(string.match("<b>text</b>", "<(.*)>"))
