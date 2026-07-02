-- v2.11 CORPUS-II: string.sub edge cases.
print(string.sub("hello", 0))       -- "hello" (0 clamps to 1)
print(string.sub("hello", 6))       -- "" (past end)
print(string.sub("hello", 3, 100))  -- "llo"
print(string.sub("hello", -10, 3))  -- "hel"
print(string.sub("hello", 0, 0))    -- ""
print(string.sub("hello", 3, 2))    -- "" (start > end)
print(string.sub("hello", -3))      -- "llo"
print(string.sub("hello", -3, -1))  -- "llo"
print(string.sub("hello", -3, -2))  -- "ll"
