-- the prototype 5.1's parser holds on the stack while load calls its
-- reader: what the language defines for such a value (PUC fails on the
-- rest, so they are not here)
local k, p = 0
load(function()
  k = k + 1
  if k == 2 then p = select(2, debug.getlocal(2, 5)) end
  return ({"local a = 1 ", "return a"})[k]
end)
print(type(p), p == p, rawequal(p, p), p == {})
local t = {} t[p] = 1 print(t[p], next(t) == p)
for _, f in ipairs {
  function() return rawget(p, 1) end,
  function() return string.rep(p, 2) end,
  function() return setmetatable(p, {}) end,
  function() return next(p) end,
  function() return ipairs(p) end,
  function() return string.format("%d", p) end,
  function() return table.concat(p) end,
} do
  local ok, e = pcall(f)
  print(ok, (string.gsub(e, "^[^:]*:%d+: ", "")))
end
