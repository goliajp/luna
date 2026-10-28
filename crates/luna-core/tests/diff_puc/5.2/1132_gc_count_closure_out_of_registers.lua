-- from 5.2 a function's closure takes a register before the function is
-- closed: out of registers there, the function is left open, with no final
-- return made, and the count read after the failed load has its arrays as
-- they stood
local load = loadstring or load
local locals = {}
for i = 1, 190 do locals[i] = "l" .. i end
local head = "local a = 1\nlocal " .. table.concat(locals, ", ") .. "\nreturn "
local parts = {}
for i = 1, 80 do parts[i] = (i % 2 == 0) and "function() end" or "a" end
local chain = table.concat(parts, " .. ")
-- one operand more or less puts the last register on a closure
local srcs = {head .. chain, head .. "a .. " .. chain}
local out = {}
coroutine.wrap(function()
  for pass = 1, 2 do
    for i, src in ipairs(srcs) do
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local before = collectgarbage("count")
      local f, e = load(src, "=chain")
      local mid = collectgarbage("count")
      collectgarbage(); collectgarbage(); collectgarbage("stop")
      local after = collectgarbage("count")
      out[i] = {f, e, mid - before, after - before}
    end
  end
end)()
for _, r in ipairs(out) do
  print(type(r[1]), r[2], string.format("%.0f %.0f", r[3] * 1024, r[4] * 1024))
end
