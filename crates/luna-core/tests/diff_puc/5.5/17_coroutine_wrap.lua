-- v2.10 CORPUS: coroutine.wrap.
local gen = coroutine.wrap(function()
  for i = 1, 3 do coroutine.yield(i * 10) end
end)
print(gen())  -- 10
print(gen())  -- 20
print(gen())  -- 30

-- generator idiom
local function range(n)
  return coroutine.wrap(function()
    for i = 1, n do coroutine.yield(i) end
  end)
end
local sum = 0
for v in range(5) do sum = sum + v end
print(sum)  -- 15
