-- v2.10 CORPUS: upvalue sharing between closures.
local function makepair()
  local n = 0
  local function get() return n end
  local function inc() n = n + 1 end
  return get, inc
end
local g, i = makepair()
print(g())  -- 0
i()
print(g())  -- 1
i(); i(); i()
print(g())  -- 4

-- closure over loop variable (each iter should get its own binding)
local fns = {}
for i = 1, 3 do
  fns[i] = function() return i end
end
print(fns[1](), fns[2](), fns[3]())  -- 1 2 3

-- closing over multiple loop vars
local pairs = {}
for a = 1, 2 do
  for b = 1, 2 do
    pairs[#pairs+1] = function() return a * 10 + b end
  end
end
for _, f in ipairs(pairs) do io.write(f(), " ") end
print()
