-- v2.13 CORPUS-IV: gsub replacement corner cases — %% in repl,
-- nil/false from function keeps original, capture index errors.
print(string.gsub("a b", " ", "%%"))
print(string.gsub("abc", "%w", function(c) if c == "b" then return "B" end end))
print(string.gsub("abc", "%w", function() return false end))
print((pcall(string.gsub, "x", "(x)", "%2")))
print(string.gsub("hello", "", "-"))
print((pcall(string.gsub, "x", "x", true)))
