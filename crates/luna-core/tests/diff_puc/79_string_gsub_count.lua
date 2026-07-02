-- v2.11 CORPUS-II: string.gsub replace count.
local r, n = string.gsub("aaaa", "a", "b")
print(r, n)  -- bbbb, 4

local r2, n2 = string.gsub("aaaa", "a", "b", 2)
print(r2, n2)  -- bbaa, 2

-- gsub with capture in replacement
local r3, n3 = string.gsub("hello world", "(%w+)", "<%1>")
print(r3, n3)  -- <hello> <world>, 2

-- %0 = whole match
local r4 = string.gsub("hello", "l+", "(%0)")
print(r4)
