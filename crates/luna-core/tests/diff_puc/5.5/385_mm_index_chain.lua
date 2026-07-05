-- v2.14 CV.3: __index chains three deep, table + function mix.
local base = { greet = "hello" }
local mid = setmetatable({ extra = 1 }, { __index = base })
local top = setmetatable({}, { __index = function(_, k)
  if k == "computed" then return 42 end
  return mid[k]
end })
print(top.greet, top.extra, top.computed, top.missing)
