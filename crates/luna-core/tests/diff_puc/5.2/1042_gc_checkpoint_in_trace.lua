local function mk(f)
  if newproxy then local o = newproxy(true); getmetatable(o).__gc = f; return o end
  return setmetatable({}, {__gc = f})
end
local fired = 0
do local x = mk(function() fired = fired + 1; local a, b, c = "x", {}, "y"; return a, b, c end) end
local ok, e = pcall(function()
  local t = {}
  for i = 1, 200000 do t[i % 100] = {} end
end)
print(fired, ok, type(e), rawequal(e, nil))
