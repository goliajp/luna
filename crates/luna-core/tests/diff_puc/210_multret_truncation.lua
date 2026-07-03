-- v2.13 CORPUS-IV: multiple-return truncation/expansion rules.
local function three() return 1, 2, 3 end
print(three())
print((three()))
print(three(), "end")
local t = { three() }
print(#t)
local u = { three(), three() }
print(#u)
local a, b, c, d = three()
print(a, b, c, d)
print("x" .. three())
