-- v2.13 CORPUS-IV: two closures share ONE upvalue cell.
local function make()
  local n = 0
  local function inc() n = n + 1 end
  local function get() return n end
  return inc, get
end
local inc, get = make()
print(get())
inc(); inc(); inc()
print(get())
local inc2, get2 = make()
print(get2(), get())
