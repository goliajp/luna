-- a chunk that parses and then fails, or fails on what the parser checks
-- beyond the grammar (a goto, a label, an assignment, an attribute, a
-- limit of the code generator), has made what PUC's parser makes up to the
-- token of the error. Read before a collection the count has them; after
-- one, the message. A chunk the parser rejects is loaded from a string and
-- read piece by piece: the reader's calls are counted among what it makes
local regs = "return " .. string.rep("1, ", 260) .. "1"
local locals = "local " .. string.rep("a, ", 210) .. "a"
-- the first `pieces_too` of them are loaded piece by piece as well
local pieces_too = 8
local bad = {
  "local c <const> = 1; c = 2",
  "local x <foo> = 1",
  "goto a\nlocal x = 1\n::a::\nreturn x",
  "::a:: ::a:: x = = 1",
  "while true do x = 1",
  "break",
  "local function f()\n  goto nope\nend\n\nreturn 1",
  "do goto a; local x = 1 ::a:: ; ; end return 1",
  regs,
  locals,
  "x = f(" .. string.rep("1, ", 260) .. "1)\nlocal = 1",
}
local function pieces(s)
  local i = 0
  return function()
    i = i + 1
    return s:sub(i * 7 - 6, i * 7) ~= "" and s:sub(i * 7 - 6, i * 7) or nil
  end
end
local keep = {""}
for i = 1, 3000 do keep[#keep + 1] = "k" .. i end
for c = 32, 126 do keep[#keep + 1] = string.char(c) end
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    for i, s in ipairs(bad) do
      for _, read in ipairs(i <= pieces_too and {false, true} or {false}) do
        local before = collectgarbage("count")
        local f, e = load(read and pieces(s) or s, "=bad" .. i)
        local mid = collectgarbage("count")
        collectgarbage(); collectgarbage(); collectgarbage("stop")
        local after = collectgarbage("count")
        out[#out + 1] = {i, read, f, e, mid - before, after - before}
      end
    end
  end
end)()
for n = math.floor(#out / 2) + 1, #out do
  local r = out[n]
  print(r[1], r[2], type(r[3]), r[4], string.format("%.0f %.0f", r[5] * 1024, r[6] * 1024))
end
