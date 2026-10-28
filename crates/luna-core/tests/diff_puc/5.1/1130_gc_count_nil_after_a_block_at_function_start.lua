-- the LOADNILs 5.1 codes for locals after a block ended at a function's
-- start grow the function's code as the load counts it
local src = "local function g()\n" .. string.rep("do local a end\n", 2000) .. "end"
collectgarbage(); collectgarbage(); collectgarbage("stop")
local before = collectgarbage("count")
local g = loadstring(src, "=many")
print(type(g), string.format("%.0f", (collectgarbage("count") - before) * 1024))
