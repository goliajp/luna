-- every arithmetic and bitwise operator over integer, float, string and
-- metatable operands: the Int/Int and Float/Float cases run in the opcode
-- arms, the rest in the slow path, and both must agree with PUC
local mt = {}
for _, e in ipairs({"add", "sub", "mul", "div", "mod", "idiv", "pow", "band", "bor", "bxor", "shl", "shr"}) do
  mt["__" .. e] = function(a, b) return e end
end
local obj = setmetatable({}, mt)
local vals = {3, -7, 0, 2.5, -0.0, 1e308, math.maxinteger, math.mininteger, -1, 64, "10", "0x10", "2.5", obj}
local ops = {
  function(a, b) return a + b end, function(a, b) return a - b end,
  function(a, b) return a * b end, function(a, b) return a / b end,
  function(a, b) return a % b end, function(a, b) return a // b end,
  function(a, b) return a ^ b end, function(a, b) return a & b end,
  function(a, b) return a | b end, function(a, b) return a ~ b end,
  function(a, b) return a << b end, function(a, b) return a >> b end,
}
for i, a in ipairs(vals) do
  for j, b in ipairs(vals) do
    local row = {}
    for _, f in ipairs(ops) do
      local ok, r = pcall(f, a, b)
      if ok then
        r = math.type(r) and (math.type(r) .. ":" .. tostring(r)) or tostring(r)
        if r:find("nan") then r = "nan" end
      else
        r = "E:" .. tostring(r):gsub("^[^:]*:%d+: ", ""):gsub(" %(.*$", "")
      end
      row[#row + 1] = r
    end
    print(i, j, table.concat(row, "|"))
  end
end
-- constant operands compile to the same register forms
local x, y = 17, -5
print(x % 3, x // 3, x % -3, x // -3, y % 3, y // 3, y % -3, y // -3)
print(x + 0.5, x * 2, x - 2.5, 1 / x, x & 6, x | 6, x ~ 6, x << 2, x >> 2)
