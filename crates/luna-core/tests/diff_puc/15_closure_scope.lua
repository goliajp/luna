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

-- vararg + closure: '...' itself cannot be captured by an inner
-- function, so materialize it into a table first.
local function collect(...)
  local args = table.pack(...)
  return function(i) return args[i] end, args.n
end
local pick, n = collect(10, 20, 30, 40)
print(pick(2), pick(4), n)
