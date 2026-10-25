-- objects with finalizers die; the collector runs one basic step at a
-- time, printing the count (from where it starts) after each and whether
-- the cycle ended, and each finalizer the step it ran at. The string
-- table grows during its sweep, which 5.2 then finishes first; the sweep
-- into the list stops at the first object of PUC's kept
collectgarbage("stop")
local base = collectgarbage("count") * 1024
local step, log = 0, {}
local function count() return string.format("%.0f", collectgarbage("count") * 1024 - base) end
local function mk(i)
  local t = setmetatable({}, {__gc = function() log[#log + 1] = i .. "@" .. step end})
  local pad = {}
  for k = 1, i % 5 do pad[k] = {} end
  return t, pad
end
for round = 1, 3 do
  local keep = {}
  for i = 1, 30 do
    local t, pad = mk(i)
    if i % 3 == 0 then keep[#keep + 1] = pad end
  end
  keep = nil
  local out = {}
  for s = 1, 400 do
    step = s
    local done = collectgarbage("step", 0)
    out[#out + 1] = count()
    if done then out[#out + 1] = "|" break end
  end
  print(round, table.concat(out, " "))
  print(round, table.concat(log, " "))
  log = {}
end
