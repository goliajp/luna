-- the cap on a function's locals: up to 5.4 every declared variable counts,
-- compile-time constants too, checked as each name is read; 5.5 counts the
-- registers the locals take, checked as they come into scope, so a constant
-- folded at compile time takes none. The message, the token it is near and
-- the count read before a collection all follow where the check stops
local load = loadstring or load
local function locals(n, fmt, from)
  local t = {}
  for i = from or 1, (from or 1) + n - 1 do t[#t + 1] = string.format(fmt, i, i) end
  return table.concat(t, "\n")
end
local plain = "local v%d = %d"
local bad = {
  locals(197, plain) .. "\nlocal x, y, z = 1, 2, 3",
  locals(198, plain) .. "\nlocal x, y, z = 1, 2, 3",
  locals(199, plain) .. "\nlocal x, y, z = 1, 2, 3",
  locals(200, plain),
  locals(201, plain),
  locals(201, plain) .. "\nx = = 1",
  locals(197, plain) .. "\nfor i = 1, 2 do local w = i end",
  locals(198, plain) .. "\nfor k, v in pairs({}) do end",
  "local function f()\n" .. locals(199, plain) .. "\nlocal a, b = 1\nend",
  locals(199, plain) .. "\nlocal function f() end",
  locals(200, plain) .. "\nlocal function f() end",
}
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    for i, s in ipairs(bad) do
      local before = collectgarbage("count")
      local f, e = load(s, "=bad" .. i)
      local mid = collectgarbage("count")
      f = nil
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local after = collectgarbage("count")
      out[#out + 1] = {i, e or "ok", mid - before, after - before}
    end
  end
end)()
for n = math.floor(#out / 2) + 1, #out do
  local r = out[n]
  print(r[1], r[2], string.format("%.0f %.0f", r[3] * 1024, r[4] * 1024))
end
