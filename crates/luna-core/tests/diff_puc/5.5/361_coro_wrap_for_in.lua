-- v2.14 CV.3: wrap as a generic-for iterator.
local function range(n)
  return coroutine.wrap(function()
    for i = 1, n do coroutine.yield(i, i * i) end
  end)
end
for i, sq in range(4) do io.write(i, ":", sq, " ") end
print()
