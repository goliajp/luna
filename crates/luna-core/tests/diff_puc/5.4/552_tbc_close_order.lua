-- v2.15 P2.5: tbc __close LIFO order in block.
local out = {}
do
  local a <close> = setmetatable({}, {__close = function() out[#out+1] = "a" end})
  local b <close> = setmetatable({}, {__close = function() out[#out+1] = "b" end})
  local c <close> = setmetatable({}, {__close = function() out[#out+1] = "c" end})
end
print(table.concat(out, ","))
