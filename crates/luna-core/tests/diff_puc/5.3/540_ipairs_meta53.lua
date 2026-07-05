-- v2.14 HD 5.3 seed: ipairs now goes through __index (the
-- 5.2-era __ipairs is deprecated); bounded __index ends at nil.
local t = setmetatable({ 1, 2 }, {
  __index = function(_, k)
    if k <= 4 then return k * 10 end
  end,
})
for i, v in ipairs(t) do io.write(i, "=", v, " ") end
print()
