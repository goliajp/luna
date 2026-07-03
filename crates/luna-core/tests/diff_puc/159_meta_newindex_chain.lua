-- v2.13 CORPUS-IV: __newindex — function form + table-forward
-- form; rawset bypass.
local store = {}
local proxy = setmetatable({}, {
  __newindex = function(t, k, v) store[k] = v end,
})
proxy.a = 1
proxy.b = 2
print(store.a, store.b, rawget(proxy, "a"))

local target = {}
local fwd = setmetatable({}, { __newindex = target })
fwd.x = "forwarded"
print(target.x, rawget(fwd, "x"))

rawset(fwd, "y", "raw")
print(rawget(fwd, "y"), target.y)

-- existing key: __newindex NOT consulted
local half = setmetatable({ k = 1 }, { __newindex = function() error("no") end })
half.k = 2
print(half.k)
