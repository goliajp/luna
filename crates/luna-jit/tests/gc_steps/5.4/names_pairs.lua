collectgarbage("incremental")
-- "__pairs", made by pairs where its last one was freed, among objects
-- made and kept meanwhile
local t, keep, out = setmetatable({1}, {}), {}, {}
for i = 1, 60000 do
  local _ = {i}
  if i % 3 == 0 then keep[#keep + 1] = {} end
  if i % 1500 == 0 then for _ in pairs(t) do end end
  if i % 500 == 0 then out[#out + 1] = string.format("%.0f", collectgarbage("count") * 1024) end
end
print(table.concat(out, " "))
