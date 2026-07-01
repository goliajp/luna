-- v2.10 CORPUS: block scoping + shadowing.
local x = 10
do
  local x = 20
  do
    local x = 30
    print(x)  -- 30
  end
  print(x)  -- 20
end
print(x)  -- 10

-- closure captures at declaration site, not call site
local function outer()
  local y = "outer_y"
  return function() return y end
end
local f = outer()
print(f())  -- outer_y

-- vararg + closure
local function collect(...)
  local n = select('#', ...)
  return function(i) return (select(i, ...)) end
end
local pick = collect(10, 20, 30, 40)
print(pick(2), pick(4))
