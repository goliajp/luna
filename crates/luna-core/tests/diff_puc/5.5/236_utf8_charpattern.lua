-- v2.13 CORPUS-IV: utf8.charpattern tokenizes multibyte text.
local s = "aé中b"
local parts = {}
for c in s:gmatch(utf8.charpattern) do parts[#parts + 1] = c end
print(#parts)
for _, c in ipairs(parts) do io.write("[", c, "]") end
print()
print(utf8.charpattern == "[\0-\x7F\xC2-\xFD][\x80-\xBF]*")
