-- v2.13 CORPUS-IV: repeat-until condition sees the body's locals.
local i = 0
repeat
  local done = i >= 3
  i = i + 1
until done
print(i)
local acc = {}
local n = 0
repeat
  local v = n * n
  acc[#acc + 1] = v
  n = n + 1
until v >= 9
print(table.concat(acc, ","))
