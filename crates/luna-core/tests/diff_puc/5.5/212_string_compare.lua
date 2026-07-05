-- v2.13 CORPUS-IV: string relational comparison (byte-wise
-- lexicographic, prefix rule).
print("a" < "b", "b" < "a", "a" < "a")
print("a" < "ab", "ab" < "b")
print("A" < "a")
print("" < "a", "" < "")
print("abc" <= "abc", "abd" >= "abc")
print((pcall(function() return "1" < 2 end)))
