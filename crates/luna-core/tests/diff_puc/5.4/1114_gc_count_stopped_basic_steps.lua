-- the collector stopped, in incremental mode, then driven by basic steps:
-- whether each finished a cycle, and the count from where it started
collectgarbage("incremental")
collectgarbage("stop")
local base = collectgarbage("count") * 1024
local keep, out = {}, {}
for i = 1, 400 do
  keep[i % 50] = {i, tostring(i)}
  local done = collectgarbage("step", 0)
  if i % 20 == 0 then
    out[#out + 1] = string.format("%s:%.0f", tostring(done), collectgarbage("count") * 1024 - base)
  end
end
print(table.concat(out, " "))
for i = 1, 10 do out[i] = tostring(collectgarbage("step", 1)) end
print(table.concat(out, " ", 1, 10), collectgarbage("isrunning"))
