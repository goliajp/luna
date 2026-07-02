-- v2.12 CORPUS-III: pcall preserves multiple returns AND handles errors.
local ok, a, b, c = pcall(function() return 10, 20, 30 end)
print(ok, a, b, c)

-- error preserves single value
local ok2, err = pcall(function() error("failure") end)
print(ok2, err:match(": (.+)$") or err)

-- pcall passing args
local ok3, sum = pcall(function(x, y) return x + y end, 3, 4)
print(ok3, sum)
