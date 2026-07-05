-- v2.13 CORPUS-IV: __mod / __idiv / __pow metamethods.
local mt
mt = {
  __mod = function(a, b) return "mod" end,
  __idiv = function(a, b) return "idiv" end,
  __pow = function(a, b) return "pow" end,
}
local o = setmetatable({}, mt)
print(o % 2, 2 % o)
print(o // 2, 2 // o)
print(o ^ 2, 2 ^ o)
