-- v2.15 P2.4 utf8: mixed content operations.
local greetings = utf8.char(72, 101, 108, 108, 111, 32, 0xe9, 0xef)
-- H, e, l, l, o, space, é, ï (but need proper encoding)
local s = "Hello " .. utf8.char(0xe9) .. utf8.char(0xef)
print(utf8.len(s))
local n = 0
for _ in utf8.codes(s) do n = n + 1 end
print(n)
