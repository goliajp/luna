-- the atomic step clears the string cache after every weak table, the
-- ones a finalizer brought back too: a long string such a table alone
-- holds is marked as the table is cleared and stays cached, so the next
-- lookup of the same C string makes no new string
local f = load("return 1", "@" .. string.rep("a", 55))
local function lookup()
  local before = collectgarbage("count")
  local s = debug.getinfo(f, "S").short_src
  return string.format("%.0f", (collectgarbage("count") - before) * 1024), s
end
for _, mode in ipairs({"kv", "v", "k"}) do
  collectgarbage()
  collectgarbage("stop")
  local _, s = lookup()
  local back
  do
    local t = setmetatable({s}, {__mode = mode})
    setmetatable({t}, {__gc = function(o) back = o[1] end})
  end
  s = nil
  collectgarbage()
  collectgarbage("stop")
  local first = lookup()
  local again = lookup()
  print(mode, first, again, #back[1])
  collectgarbage("restart")
end
