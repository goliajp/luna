-- v2.13 CORPUS-IV: __concat — fires when either operand is
-- non-string/number; right-associative resolution order.
local mt = {
  __concat = function(a, b)
    local an = type(a) == "table" and "obj" or tostring(a)
    local bn = type(b) == "table" and "obj" or tostring(b)
    return "<" .. an .. "|" .. bn .. ">"
  end,
}
local o = setmetatable({}, mt)
print(o .. "x")
print("x" .. o)
print(1 .. o)
print("a" .. "b" .. o)
print(o .. "a" .. "b")
