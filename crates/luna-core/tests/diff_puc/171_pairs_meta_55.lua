-- v2.13 CORPUS-IV: __pairs is consulted by pairs() in 5.5
-- (removed in 5.4, restored in 5.5) — custom iterator drives
-- the loop instead of raw entries.
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
for k, v in pairs(t) do
  parts[#parts + 1] = k .. "=" .. v
end
print(#parts, table.concat(parts, ","))
-- without __pairs: raw iteration
local plain = { only = "raw" }
for k, v in pairs(plain) do print(k, v) end
