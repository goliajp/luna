-- 5.1 (LUA_COMPAT_VARARG) declares a local `arg` after the fixed
-- parameters of every vararg function; it holds a table of the extra
-- arguments only when the body does not use `...`, and is nil otherwise
arg = 'global'
local function uses(...)
  local n = select('#', ...)
  return arg, n
end
print('uses', uses(1, 2))
local function nouse(a, ...)
  return type(arg), arg and arg.n, arg and arg[2]
end
print('nouse', nouse(1, 2, 3))
local function names(a, ...)
  local l = 'l'
  local out = {}
  for i = 1, 8 do
    local n, v = debug.getlocal(1, i)
    if not n then break end
    if n:sub(1, 1) ~= '(' then out[#out + 1] = n .. '=' .. tostring(type(v)) end
  end
  return table.concat(out, ' '), select('#', ...)
end
print('names', names(1, 2))
local function nested(...)
  local inner = function(...) return ... end
  return type(arg)
end
print('nested', nested(1))
