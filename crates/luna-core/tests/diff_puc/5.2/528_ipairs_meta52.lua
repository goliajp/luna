-- v2.14 HD 5.2 seed: 5.2's ipairs consults the __ipairs
-- metamethod (added 5.2, deprecated 5.3, removed 5.4).
local t = setmetatable({}, {
  __ipairs = function(self)
    local i = 0
    return function()
      i = i + 1
      if i <= 3 then return i, i * 100 end
    end, self, 0
  end,
})
for i, v in ipairs(t) do io.write(i, "=", v, " ") end
print()
