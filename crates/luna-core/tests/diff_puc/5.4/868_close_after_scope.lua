-- 5.4 ends a block's variables before its CLOSE runs (`leaveblock` calls
-- `removevars` first, and `break` closes at the loop's "break" label), so
-- a __close handler that reads the closing frame with debug.getlocal sees
-- "(temporary)" there; a loop variable outlives its body's close.
local function names(level, a, b)
  return (debug.getlocal(level + 1, a)), (debug.getlocal(level + 1, b))
end
local function closer(a, b)
  return setmetatable({}, {__close = function() print(names(2, a, b)) end})
end
local function f()
  do
    local x = 1
    local c <close> = closer(1, 2)
  end
  local y = 5
  do
    local z = 2
    local g = function() return z end
    local c <close> = closer(2, 3)
  end
  while true do
    local w = 3
    local c <close> = closer(2, 3)
    if w then break end
  end
  for i = 1, 1 do
    local c <close> = closer(5, 6)
  end
  local n = 0
  repeat
    local r = n
    local c <close> = closer(2, 3)
    n = n + 1
  until n == 2
  local function h()
    local q = 1
    local c <close> = closer(1, 2)
    return 7
  end
  print(h(), y)
end
f()
