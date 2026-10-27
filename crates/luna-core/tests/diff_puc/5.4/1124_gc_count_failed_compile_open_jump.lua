-- a chunk that fails to compile inside a loop has made the loop's
-- instructions up to the error, though the loop's jump never got its
-- target: the count read before a collection has them
local names = "local " .. string.rep("a, ", 210) .. "a"
local bad = {
  "for i = 1, 2 do " .. names .. " end",
  "while c do " .. names .. " end",
}
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    for i, s in ipairs(bad) do
      local before = collectgarbage("count")
      local f, e = load(s, "=bad" .. i)
      local mid = collectgarbage("count")
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local after = collectgarbage("count")
      out[#out + 1] = {i, f, e, mid - before, after - before}
    end
  end
end)()
for n = #out // 2 + 1, #out do
  local r = out[n]
  print(r[1], type(r[2]), r[3], string.format("%.0f %.0f", r[4] * 1024, r[5] * 1024))
end
