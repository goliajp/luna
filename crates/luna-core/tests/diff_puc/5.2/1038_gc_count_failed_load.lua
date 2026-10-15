-- a chunk that fails to compile has made what PUC's parser makes up to the
-- token of the error: the main function, the prototypes opened so far with
-- their arrays, the parser's tables, the scanner's strings, the message.
-- Read before a collection the count has them; after one, the message
local ld = loadstring or load
local bad = {
  ")",
  "local x = 1 +",
  "local t = {1, 2, 3}\nlocal s = 'abc' .. 'def'\nif t then s = 1 else",
  "local function f(a, b)\n  local c = a + b\n  return function() return c end\nend\nf(1, 2) = 3",
  "local function g()\n  local u = 'unfinished\nend",
  "x = 3x4",
  "for i = 1, 10 do print(i) )",
  "local a = {[1] = 2,",
  "return 'a string that does not end",
  "local function h(p)\n  if p then\n    return function(q)\n      while q do q = q - 1 end\n      return q +\n    end\n  end\nend",
  "local s = [[long string\nthat does not end",
  "--[[ a comment\nthat does not end",
  "x = 'bad escape \\q here'",
  "local veryLongVariableNameNumberOne, veryLongVariableNameNumberTwo = 1, 2\n" ..
    string.rep("veryLongVariableNameNumberOne = veryLongVariableNameNumberTwo + 1\n", 20) .. "end",
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
