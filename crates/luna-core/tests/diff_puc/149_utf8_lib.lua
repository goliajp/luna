-- v2.12 CORPUS-III: utf8 library basics (5.3+).
local s = utf8.char(72, 105, 0x4E2D)
print(s, #s, utf8.len(s))
print(utf8.codepoint(s, 1, -1))
print(utf8.offset(s, 2), utf8.offset(s, 3))
for p, c in utf8.codes("ab") do print(p, c) end
