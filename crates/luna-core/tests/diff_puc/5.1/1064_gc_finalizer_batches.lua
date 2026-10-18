-- fifty objects with finalizers dropped at once, then garbage made a
-- little at a time: how many finalizers had run at each allocation
-- that ran some
local ran = 0
local base = newproxy(true)
debug.getmetatable(base).__gc = function() ran = ran + 1 end
collectgarbage()
collectgarbage()
do
  for _ = 1, 50 do newproxy(base) end
end
local log = {}
local last = 0
for i = 1, 20000 do
  local _ = {i}
  if ran ~= last then
    log[#log + 1] = i .. ":" .. ran
    last = ran
  end
  if ran == 50 then break end
end
print(table.concat(log, " "))
