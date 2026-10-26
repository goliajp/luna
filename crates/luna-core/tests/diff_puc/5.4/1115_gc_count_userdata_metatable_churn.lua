-- io.stdout given a new metatable again and again, the collector running:
-- lua_setmetatable marks a userdata's new metatable (luaC_objbarrier),
-- which keeps it alive through the cycle under way
collectgarbage("incremental")
local base = collectgarbage("count") * 1024
local out = {}
for i = 1, 3000 do
  debug.setmetatable(io.stdout, {__index = {i = i}, __gc = function() end})
  if i % 300 == 0 then
    out[#out + 1] = string.format("%.0f", collectgarbage("count") * 1024 - base)
  end
end
collectgarbage()
out[#out + 1] = string.format("%.0f", collectgarbage("count") * 1024 - base)
print(table.concat(out, " "))
