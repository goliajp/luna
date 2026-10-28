-- from 5.4 an instruction is saved on the line it is coded on, then a call
-- or an operator moves it to its own line: coded 128 or more lines away
-- from the line before, it takes an absolute line entry, and the move a
-- second one; the count read after each load has them
local nl = string.rep("\n", 200)
-- (no function closes before the code measured: from 5.5 the keys a
-- closed function leaves dead in the chunk's table are reused by address)
local pre = "local a, t, z, g, h = 1, {}\n"
local srcs = {
  "local f = assert(load([[" .. nl .. "]]))",
  "local t = g(h({" .. nl .. "}))",
  "local s = g(h([[" .. nl .. "]]), 1)",
  "local x = a + g(" .. nl .. "1)",
  "local y = -g(" .. nl .. ")",
  "for i = 1, 2 do" .. nl .. "end",
  "local s = [[" .. nl .. "]] .. g()",
  "local z = #" .. nl .. "t + 1",
  "local z = g" .. nl .. "(a) + g(1)",
  "function" .. nl .. "g2() return 1 end",
  "local function" .. nl .. "h" .. nl .. "() return 1" .. nl .. "end",
  "local w = function" .. nl .. "(...)" .. nl .. "return ... end",
  "for" .. nl .. "i = 1, 2 do end",
  "for k in" .. nl .. "pairs(t)" .. nl .. "do end",
  "local x = a +" .. nl .. "h local y = a",
  "local x = a + g(" .. nl .. "1) local y = a",
  "local w = function(" .. nl .. "...) return ... end",
}
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    for i, src in ipairs(srcs) do
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local before = collectgarbage("count")
      local f = assert(load(pre .. src, "=far"))
      local mid = collectgarbage("count")
      out[i] = string.format("%.0f", (mid - before) * 1024)
      f = nil
    end
  end
end)()
print(table.concat(out, " "))
