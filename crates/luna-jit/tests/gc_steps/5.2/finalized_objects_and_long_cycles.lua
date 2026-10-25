-- objects with finalizers die among many others over long cycles of
-- basic steps: the count (from where it starts) after each step, and the
-- step each finalizer ran at
collectgarbage("stop")
local base = collectgarbage("count") * 1024
local step, log = 0, {}
local function count() return string.format("%.0f", collectgarbage("count") * 1024 - base) end
for round = 1, 4 do
  local keep = {}
  for i = 1, 60 do
    setmetatable({string.rep("f", 100 + i)}, {__gc = function() log[#log + 1] = i .. "@" .. step end})
    for k = 1, 4 do keep[#keep + 1] = {k} end
    keep[#keep + 1] = string.rep("k", 200 + i)
  end
  if round % 2 == 0 then keep = nil end
  local out = {}
  for s = 1, 2000 do
    step = s
    local done = collectgarbage("step", 0)
    out[#out + 1] = count()
    if done then out[#out + 1] = "|" break end
  end
  print(round, table.concat(out, " "))
  print(round, table.concat(log, " "))
  log = {}
end
