-- a function given more parameters than a function takes locals has been
-- opened when the one too many is declared (up to 5.4 as its name is read,
-- 5.5 at the `)`, once each has its debug record): the count read before a
-- collection has the prototype and what the parameters made
local load = loadstring or load
local function names(n)
  local t = {}
  for i = 1, n do t[i] = "a" .. i end
  return table.concat(t, ", ")
end
local bad = {
  "function f(" .. names(260) .. ") end",
  "function t:m(" .. names(200) .. ") end",
  "local f = function(" .. names(200) .. ", ...) end",
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
for n = math.floor(#out / 2) + 1, #out do
  local r = out[n]
  print(r[1], type(r[2]), r[3], string.format("%.0f %.0f", r[4] * 1024, r[5] * 1024))
end
