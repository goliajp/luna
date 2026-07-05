-- v2.11 CORPUS-II: string.find with captures returns start, end, cap1, cap2..
local s, e, cap1, cap2 = string.find("hello 42 world", "(%a+) (%d+)")
print(s, e, cap1, cap2)

-- init position
local s2, e2 = string.find("abcabc", "abc", 2)
print(s2, e2)

-- plain=true
print(string.find("a.b.c", ".", 1, true))
print(string.find("a.b.c", ".", 1))  -- pattern: any char
