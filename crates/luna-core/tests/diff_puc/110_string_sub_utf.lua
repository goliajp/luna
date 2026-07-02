-- v2.12 CORPUS-III: string.sub byte-level semantics.
local s = "café"
print(#s)      -- byte-len (5 for 'c' 'a' 'f' 'é'=2bytes)
print(string.sub(s, 1, 3))   -- first 3 bytes: caf
print(string.sub(s, -2))      -- last 2 bytes: é
