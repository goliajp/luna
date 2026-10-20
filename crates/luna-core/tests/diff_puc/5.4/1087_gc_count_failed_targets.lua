-- a chunk whose assignment target is no variable (`(a) = 1`, `a, f() =`),
-- or that ends after a binary operator, has made what PUC's parser makes
-- up to the token of the error: the target's code, the operand's constant
-- and the parser's tables. Read before a collection the count has them
local ld = loadstring or load
local bad = {
  "(a) = 1",
  "a, f() = 1, 2",
  "x = 1\ny = [[\n\n]] +",
  "local t = {}\nt.x, (t) = 1, 2",
  "(f)() = 1",
  "local s = 'k' ..",
  "z = z and",
  "w = {} .. #",
}
-- (printed at the end: what printing makes is not in the counts). Strings
-- kept for the whole run, the empty one and every one-character one among
-- them, so that the string table does not grow while the loads run and the
-- messages' pieces exist whatever else the host has made
local keep = {""}
for i = 1, 3000 do keep[#keep + 1] = "k" .. i end
for c = 32, 126 do keep[#keep + 1] = string.char(c) end
-- measured twice, the second time printed: by then the strings the
-- messages are made of exist already, as they may in whatever host runs
-- this; in a coroutine, whose stack is the same whatever calls this
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    for i, s in ipairs(bad) do
      local before = collectgarbage("count")
      local f, e = ld(s, "=bad" .. i)
      local mid = collectgarbage("count")
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local after = collectgarbage("count")
      out[i] = {f, e, mid - before, after - before}
    end
  end
end)()
for i, r in ipairs(out) do
  print(i, type(r[1]), r[2], string.format("%.0f %.0f", r[3] * 1024, r[4] * 1024))
end
