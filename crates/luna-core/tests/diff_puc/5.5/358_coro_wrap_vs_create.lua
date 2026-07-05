-- v2.14 CV.3: wrap returns values directly and re-raises errors.
local gen = coroutine.wrap(function()
  coroutine.yield(1)
  coroutine.yield(2, 3)
  return "end"
end)
print(gen())
print(gen())
print(gen())
local bad = coroutine.wrap(function() error("wrapped", 0) end)
print(pcall(bad))
