-- the atomic step clears the string cache after every weak table, the
-- ones a finalizer brought back too: a long string such a table alone
-- holds is marked as the table is cleared and stays cached, so the next
-- lookup of the same C string makes no new string
-- the cache finds a string by its C address, and debug.getinfo's
-- short_src is a buffer on the C stack: another cached string sharing the
-- cache row may push it out, as where the system puts the stack and the
-- program decides; each round runs one C call deeper, and the least of
-- what the rounds count is printed
local f = load("return 1", "@" .. string.rep("a", 55))
local function lookup()
  local before = collectgarbage("count")
  local s = debug.getinfo(f, "S").short_src
  return (collectgarbage("count") - before) * 1024, s
end
local function round(mode)
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
  collectgarbage("restart")
  return first, again, #back[1]
end
local function at(depth, mode)
  if depth == 0 then
    return round(mode)
  end
  return select(2, pcall(at, depth - 1, mode))
end
for _, mode in ipairs({"kv", "v", "k"}) do
  local first, again, len
  for depth = 0, 3 do
    local a, b, n = at(depth, mode)
    first = math.min(first or a, a)
    again = math.min(again or b, b)
    len = n
  end
  print(mode, string.format("%.0f", first), string.format("%.0f", again), len)
end
