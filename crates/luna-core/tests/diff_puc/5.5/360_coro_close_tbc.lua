-- v2.14 CV.3: close runs pending to-be-closed variables.
local log = {}
local co = coroutine.create(function()
  local guard <close> = setmetatable({}, {
    __close = function() log[#log + 1] = "closed" end,
  })
  coroutine.yield()
  log[#log + 1] = "never"
end)
coroutine.resume(co)
print(#log)
print(coroutine.close(co))
print(table.concat(log, ","))
