-- v2.12 CORPUS-III: repeat...until has local visible in until expr.
local n = 0
repeat
  n = n + 1
  local should_stop = n >= 3
until should_stop
print(n)   -- 3
