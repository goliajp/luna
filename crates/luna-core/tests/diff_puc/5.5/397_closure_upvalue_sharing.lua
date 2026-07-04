-- v2.14 CV.3: sibling closures share upvalue cells; loop vars don't.
local function counter()
  local n = 0
  return function() n = n + 1 return n end, function() return n end
end
local inc, get = counter()
inc(); inc()
print(get())
local fns = {}
for i = 1, 3 do fns[i] = function() return i end end
print(fns[1](), fns[2](), fns[3]())
