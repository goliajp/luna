-- v3.1 debug slice: "C stack overflow" raised while a C function (pcall,
-- a gsub callback's caller) is running carries no position.
local function deep(n)
  local ok, r = pcall(deep, n + 1)
  if not ok then return r end
  return r
end
print(deep(0))
local function g(n) return (string.gsub("a", "a", function() return g(n + 1) end)) end
print(pcall(g, 0))
