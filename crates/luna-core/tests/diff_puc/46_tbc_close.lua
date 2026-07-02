-- v2.11 CORPUS-II: tbc __close (Lua 5.4+).
local order = {}
do
  local x <close> = setmetatable({}, {__close = function() order[#order+1] = "x" end})
  local y <close> = setmetatable({}, {__close = function() order[#order+1] = "y" end})
end
print(table.concat(order, ","))  -- y,x (LIFO)
