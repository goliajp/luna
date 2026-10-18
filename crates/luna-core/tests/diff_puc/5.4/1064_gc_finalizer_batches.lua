-- fifty objects with finalizers dropped at once, then garbage made a
-- little at a time: how many finalizers had run at each allocation
-- that ran some, counted from the first such allocation (where the
-- first one falls moves with PUC's string hash seed)
-- the standalone interpreter starts in generational mode
collectgarbage("incremental", 0, 100, 0)
local ran = 0
local mt = {__gc = function() ran = ran + 1 end}
collectgarbage()
collectgarbage()
do
  for _ = 1, 50 do setmetatable({}, mt) end
end
local log = {}
local last, first = 0, nil
for i = 1, 20000 do
  local _ = {i}
  if ran ~= last then
    first = first or i
    log[#log + 1] = (i - first) .. ":" .. ran
    last = ran
  end
  if ran == 50 then break end
end
print(table.concat(log, " "))
