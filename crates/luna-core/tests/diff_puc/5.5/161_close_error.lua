-- v2.13 CORPUS-IV: error thrown from __close propagates; value
-- observed through pcall.
local ok, err = pcall(function()
  local x <close> = setmetatable({}, {
    __close = function() error("close_boom", 0) end,
  })
end)
print(ok, err)
-- __close receives the pending error as second arg
local seen
local ok2 = pcall(function()
  local y <close> = setmetatable({}, {
    __close = function(_, e) seen = e end,
  })
  error("pending_err", 0)
end)
print(ok2, seen)
