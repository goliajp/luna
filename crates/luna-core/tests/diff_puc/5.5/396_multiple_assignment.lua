-- v2.14 CV.3: multi-assignment adjustment + swap + call spread.
local a, b, c = 1, 2
print(a, b, c)
a, b = b, a
print(a, b)
local function three() return 10, 20, 30 end
local x, y = three()
print(x, y)
local p, q, r, s = three(), "tail"
print(p, q, r, s)
local t = { three() }
print(#t)
local u = { three(), "cut" }
print(#u)
