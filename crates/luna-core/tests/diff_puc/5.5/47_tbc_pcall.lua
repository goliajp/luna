-- v2.11 CORPUS-II: tbc runs even on error.
local closed = {}
local ok = pcall(function()
  local a <close> = setmetatable({}, {__close = function() closed[#closed+1] = "A" end})
  error("boom")
end)
print(ok, table.concat(closed, ","))
