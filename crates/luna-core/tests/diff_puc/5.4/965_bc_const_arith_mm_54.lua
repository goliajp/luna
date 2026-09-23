-- v3.1 bytecode: arithmetic and comparisons with constant and immediate
-- operands call metamethods with the operator and operand order the source
-- wrote, not the rewritten immediate form.
local log = {}
local mt = {}
for _, e in ipairs({"add", "sub", "mul", "div", "mod", "pow", "idiv",
                    "band", "bor", "bxor", "shl", "shr"}) do
  mt["__" .. e] = function(a, b)
    return e .. "(" .. tostring(type(a) == "table" and "x" or a) .. ","
      .. tostring(type(b) == "table" and "x" or b) .. ")"
  end
end
mt.__lt = function(a, b)
  log[#log + 1] = "lt(" .. (type(a) == "table" and "x" or tostring(a)) .. ","
    .. (type(b) == "table" and "x" or tostring(b)) .. ")"
  return true
end
mt.__le = function(a, b)
  log[#log + 1] = "le(" .. (type(a) == "table" and "x" or tostring(a)) .. ","
    .. (type(b) == "table" and "x" or tostring(b)) .. ")"
  return false
end
local x = setmetatable({}, mt)
print(x + 1, 1 + x, x - 1, 1 - x, x * 2.5, 2.5 * x, x / 2, x % 3, x ^ 2)
print(x // 4, 4 // x, x & 1, 1 & x, x | 2, x ~ 3, x << 1, 1 << x, x >> 2, 2 >> x)
print(x - 200, x + 200)
local r = {x < 1, 1 < x, x <= 2, 2 <= x, x > 3, x >= 4.0, 5.0 < x}
print(table.concat(log, " "), #r)
local n = 5
print(n + 1, n - 1, 1 - n, n << 2, n >> 1, 1 << n, n * 0.5, n == 5, n == 5.0, n ~= 6)
