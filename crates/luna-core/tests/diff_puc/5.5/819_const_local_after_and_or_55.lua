-- `local x <const> = X and nil or K` is the compile-time constant K, as
-- PUC's code generator leaves the `or` with K once X is compiled: X still
-- runs, but x is not a variable (no debug.getlocal entry, no upvalue)
local calls = 0
local function f() calls = calls + 1 return true end
local function g() calls = calls + 10 return false end
local function locals()
  local names = {}
  local i = 1
  while true do
    local name = debug.getlocal(2, i)
    if not name then break end
    if name:sub(1, 1) ~= "(" then names[#names + 1] = name end
    i = i + 1
  end
  return table.concat(names, ",")
end
do
  local x <const> = f() and nil or 1
  local y <const> = g() and false or 2
  local z <const> = (f() and nil or 3) + 4
  local w <const> = f() and 5 or 6
  print(x, y, z, w, calls, locals())
  local function inner() return x + y + z end
  print(inner(), debug.getupvalue(inner, 1))
end
