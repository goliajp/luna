-- a load that fails with a runtime error ("C stack overflow" in a deep
-- expression) runs the message handler where the error is raised, inside
-- the parser: what the parser made so far is still held there, and a
-- collection in the handler leaves it
local src = "local a, b, c = 1, 2, 3\nlocal function f() return a + b end\nx = " ..
  string.rep("(", 300) .. "1" .. string.rep(")", 300)
collectgarbage(); collectgarbage()
local before = collectgarbage("count")
local inside
print(xpcall(function()
  local f, e = load(src, "=c")
  return f, e
end, function(m)
  collectgarbage()
  inside = collectgarbage("count") - before
  return "H:" .. m
end))
print(string.format("%.0f", inside * 1024))
