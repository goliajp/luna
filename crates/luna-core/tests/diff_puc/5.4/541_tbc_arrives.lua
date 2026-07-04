-- v2.14 HD 5.4 seed: to-be-closed variables arrive.
local order = {}
do
  local a <close> = setmetatable({}, { __close = function() order[#order + 1] = "a" end })
  local b <close> = setmetatable({}, { __close = function() order[#order + 1] = "b" end })
end
print(table.concat(order, ","))
