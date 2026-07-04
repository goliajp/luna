-- v2.11 CORPUS-II: vararg forwarding.
local function head(x, ...) return x end
local function tail(_, ...) return ... end
print(head(10, 20, 30))
print(tail(10, 20, 30))

local function double(...)
  return ..., ...    -- first ... adjusted to 1, second spreads
end
print(double(1, 2, 3))  -- 1 1 2 3

-- adjust-to-list at end
local function collect(...)
  local t = {}
  for i = 1, select("#", ...) do t[i] = (select(i, ...)) end
  return t
end
local c = collect("a", "b", "c")
print(c[1], c[2], c[3])
