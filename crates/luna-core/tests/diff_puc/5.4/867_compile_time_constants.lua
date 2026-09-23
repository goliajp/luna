-- 5.4+ compile-time constants: a <const> local whose value the parser
-- can compute is not a variable — no register, no debug entry, no
-- upvalue — while one holding a table or a call result is.
local function locals()
  local t, i = {}, 1
  while true do
    local n = debug.getlocal(2, i)
    if not n then break end
    t[#t + 1] = n
    i = i + 1
  end
  return table.concat(t, ",")
end
local function ups(f)
  local t, i = {}, 1
  while debug.getupvalue(f, i) do t[#t + 1] = (debug.getupvalue(f, i)) i = i + 1 end
  return table.concat(t, ",")
end
local function body()
  local a <const> = 10
  local b <const> = a * 2 + 1
  local s <const> = "str"
  local n <const> = nil
  local t <const> = {}
  local c <const> = 2 ^ 10
  local z <const> = -0.0
  print(locals(), a, b, s, n, c, 1 / z)
  local function inner() return a + b, s end
  print(ups(inner), inner())
  print(pcall(load("local x <const> = nil return x + 1", "=c")))
end
body()
