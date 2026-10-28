-- a function that names more upvalues than a function takes: the load
-- fails once the one too many is named, with the functions around it open
-- and what was read before made (5.1 shows no token for the error)
local load = loadstring or load
local function list(n, p, sep)
  local t = {}
  for i = 1, n do t[i] = p .. i end
  return table.concat(t, sep)
end
local src = "local " .. list(150, "u", "\nlocal ") .. "\nfunction g()\nlocal "
  .. list(150, "w", "\nlocal ") .. "\nfunction f() return "
  .. list(150, "u", ", ") .. ",\n" .. list(150, "w", ", ") .. " end end"
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    local before = collectgarbage("count")
    local f, e = load(src, "=bad")
    local mid = collectgarbage("count")
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    local after = collectgarbage("count")
    out[pass] = {f, e, mid - before, after - before}
  end
end)()
local r = out[2]
print(type(r[1]), r[2], string.format("%.0f %.0f", r[3] * 1024, r[4] * 1024))
