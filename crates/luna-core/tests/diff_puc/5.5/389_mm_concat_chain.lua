-- v2.14 CV.3: __concat right-associativity and side pick.
local mt = { __concat = function(x, y)
  local xs = type(x) == "table" and "T" or tostring(x)
  local ys = type(y) == "table" and "T" or tostring(y)
  return "<" .. xs .. "+" .. ys .. ">"
end }
local t = setmetatable({}, mt)
print("a" .. t)
print(t .. "b")
print("a" .. "b" .. t)
print(1 .. t)
