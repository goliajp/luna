-- v2.15 P2.4 utf8: codes() over multibyte content.
local s = "aébc"
local n = 0
for pos, cp in utf8.codes(s) do n = n + 1 end
print(n)
