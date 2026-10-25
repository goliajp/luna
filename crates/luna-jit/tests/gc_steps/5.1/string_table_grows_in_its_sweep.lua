-- new strings made one at a time with a basic step between them: the
-- string table must grow while its sweep is under way now and then, and
-- 5.1 does not resize it then (luaS_resize returns while the collector
-- sweeps strings); the count, from where it starts, every 50 strings
collectgarbage("stop")
local base = collectgarbage("count") * 1024
local keep, out = {}, {}
for i = 1, 3000 do
  keep[i] = "s" .. i
  if i % 2 == 0 then
    collectgarbage("step", 0)
    if i % 50 == 0 then
      out[#out + 1] = string.format("%.0f", collectgarbage("count") * 1024 - base)
    end
  end
end
print(table.concat(out, " "))
