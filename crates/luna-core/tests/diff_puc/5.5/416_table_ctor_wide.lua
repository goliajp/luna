-- v2.15 P2.4: wide table constructor.
local t = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20}
print(#t, t[1], t[10], t[20])
local m = {a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, g = 7, h = 8, i = 9, j = 10}
local keys = {"a", "b", "c", "d", "e", "f", "g", "h", "i", "j"}
local s = 0
for _, k in ipairs(keys) do s = s + m[k] end
print(s)
