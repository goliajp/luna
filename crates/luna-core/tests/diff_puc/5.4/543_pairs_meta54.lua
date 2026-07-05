-- v2.14 HD 5.4 seed: pairs consults __pairs in the OFFICIAL 5.4
-- build — the manual lists __pairs as removed in 5.4, but the
-- stock Makefile compiles with LUA_COMPAT_5_3 which keeps it
-- alive. The diff ground truth is the default build's behavior.
local t = setmetatable({ real = 1 }, {
  __pairs = function(self)
    local i = 0
    return function()
      i = i + 1
      if i <= 3 then return "k" .. i, i * 10 end
    end, self, nil
  end,
})
local parts = {}
for k, v in pairs(t) do parts[#parts + 1] = k .. "=" .. v end
print(#parts, table.concat(parts, ","))
