-- v2.2 Phase 5 (DP) deterministic diff fixture: closures + upvalues.
local function make_counter()
  local n = 0
  return function()
    n = n + 1
    return n
  end
end

local c1 = make_counter()
local c2 = make_counter()
print(c1(), c1(), c1())
print(c2(), c2())
print(c1())

-- upvalue capture from a loop iteration
local fns = {}
for i = 1, 3 do
  fns[i] = function() return i end
end
print(fns[1](), fns[2](), fns[3]())

-- closure as table method
local M = {n = 0}
function M:inc()
  self.n = self.n + 1
  return self.n
end
print(M:inc(), M:inc(), M:inc())
