-- v2.15 P2.5: tbc __close all fire even on error inside block.
local closed = {}
local ok = pcall(function()
  local a <close> = setmetatable({}, {__close = function() closed[#closed+1] = "a" end})
  local b <close> = setmetatable({}, {__close = function() closed[#closed+1] = "b" end})
  error("boom")
end)
print(ok, table.concat(closed, ","))
