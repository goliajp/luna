-- v2.15 P2.5 (5.5): coroutine ping-pong via channel.
local out = {}
local producer = coroutine.create(function()
  for i = 1, 5 do coroutine.yield(i) end
end)
local consumer = coroutine.create(function()
  while true do
    local _, v = coroutine.resume(producer)
    if not v then break end
    out[#out+1] = v * 10
    coroutine.yield()
  end
end)
for _ = 1, 5 do coroutine.resume(consumer) end
print(table.concat(out, ","))
