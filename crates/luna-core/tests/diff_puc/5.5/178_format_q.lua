-- v2.13 CORPUS-IV: string.format %q — reload round-trip for
-- strings with specials, integers, and floats.
local s = "a\nb\0c\"d\\e"
local q = string.format("%q", s)
local back = load("return " .. q)()
print(back == s, #back)
print(string.format("%q", 42))
print(load("return " .. string.format("%q", 0.5))() == 0.5)
print(load("return " .. string.format("%q", math.maxinteger))() == math.maxinteger)
