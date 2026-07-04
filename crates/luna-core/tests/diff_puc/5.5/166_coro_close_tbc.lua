-- v2.13 CORPUS-IV: coroutine.close runs pending <close> vars of
-- a suspended coroutine (5.4+).
local closed = {}
local co = coroutine.create(function()
  local a <close> = setmetatable({}, {
    __close = function() closed[#closed + 1] = "a" end,
  })
  local b <close> = setmetatable({}, {
    __close = function() closed[#closed + 1] = "b" end,
  })
  coroutine.yield()
  closed[#closed + 1] = "never"
end)
coroutine.resume(co)
print(coroutine.status(co), #closed)
local ok = coroutine.close(co)
print(ok, coroutine.status(co))
print(table.concat(closed, ","))
