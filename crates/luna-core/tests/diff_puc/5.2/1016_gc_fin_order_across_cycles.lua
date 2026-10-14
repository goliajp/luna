-- rule: lgc.separatetobefnz.2
-- only the explicit collections below run, whatever the automatic pacing
collectgarbage("stop")
local out = {}
local objs = {}
for i, n in ipairs{"a","b","c","d","e"} do
  objs[n] = setmetatable({}, {__gc = function() out[#out+1] = n end})
end
objs.c = nil
collectgarbage(); collectgarbage()
out[#out+1] = "|"
objs = nil
collectgarbage(); collectgarbage()
print(table.concat(out, " "))
