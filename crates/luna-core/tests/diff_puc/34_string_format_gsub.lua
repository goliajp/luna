-- v2.10 CORPUS: string.gsub replace function.
print(string.gsub("hello", "l", function(m) return m:upper() end))
print(string.gsub("hello world", "%w+", function(w) return "<"..w..">" end))
print(string.gsub("abc123def", "(%a+)(%d+)", function(a, n) return n..a end))
-- limit
print(string.gsub("aaaa", "a", "b", 2))
