-- v2.15 P2.5 (5.1): local arg = table.pack-style.
local function f(...)
  local arg = {...}
  return arg[1], arg[2], arg[3]
end
print(f(10, 20, 30))
