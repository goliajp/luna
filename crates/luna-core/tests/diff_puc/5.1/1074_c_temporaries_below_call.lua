-- the values a protected call leaves below the function it calls; below a
-- 5.4+ vararg function also its old slot, its erased fixed parameters and
-- its extra arguments
local function show(n)
  for i = 1, n do
    local name, v = debug.getlocal(3, i)
    if not name then print(i, "none") break end
    print(i, name, type(v) == "string" and v or type(v))
  end
  print("--")
end
pcall(function(...) show(6) end, "x", "y")
xpcall(function(p, ...) show(7) end, print, "x", "y", "z")
pcall(function(a, b) show(4) end, "x", "y")
pcall(function(...) show(3) end)
pcall(function(a, ...) show(5) end, "x")
