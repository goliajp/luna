-- v2.13 CORPUS-IV: anchored gsub + replacement forms (string
-- with %0/%1, table, function) + max-n.
print(string.gsub("aaa", "^a", "X"))
print(string.gsub("banana", "an", "AN", 1))
print(string.gsub("hello world", "(%w+)", "<%1>"))
print(string.gsub("abc", "%w", "%0%0"))
print(string.gsub("k1 k2", "%w+", { k1 = "v1", k2 = "v2" }))
print(string.gsub("1 2 3", "%d", function(d) return d * 2 end))
print(string.gsub("xyz", "%d", "n"))
