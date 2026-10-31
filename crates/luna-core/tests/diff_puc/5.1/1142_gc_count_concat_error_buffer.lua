-- a library buffer that pushed pieces past its own block, abandoned by an
-- error: the pieces are no longer roots once the protected call returns
collectgarbage("stop")
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local t = {}
for i = 1, 40 do t[i] = string.rep(string.char(65 + i % 26), 1000 + i) end
t[41] = {}
local ok, err = pcall(table.concat, t, "-")
print(ok, err)
t = nil
print(count() - base)
local u = {}
for i = 1, 30 do u[i] = string.rep("x", 600 + i) end
u[31] = true
print(pcall(table.concat, u))
u = nil
print(count() - base)
