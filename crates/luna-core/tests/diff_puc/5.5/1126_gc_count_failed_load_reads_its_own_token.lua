-- a load that fails on a token the parser takes no more locals at has
-- scanned the source up to that token: the token there is the source's
-- own, not the one a completion of the source would put in its place
local load = loadstring or load
local names = "local " .. string.rep("a, ", 210) .. "a"
local bad = {
  "if c then x = 1 else " .. names .. " end",
  "for i = 1, 2 do " .. names .. " end",
  "do " .. names .. " end",
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
