-- v2.13 CORPUS-IV: __index chain — table -> table -> function.
local base = setmetatable({}, {
  __index = function(_, k) return "fn:" .. k end,
})
local mid = setmetatable({ m = "mid_val" }, { __index = base })
local top = setmetatable({ t = "top_val" }, { __index = mid })
print(top.t)
print(top.m)
print(top.anything)
print(rawget(top, "m"))
