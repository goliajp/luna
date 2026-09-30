-- every arithmetic operator over number, string and metatable operands:
-- the fast cases run in the opcode arms, the rest in the slow path, and
-- both must agree with PUC (no huge operands: C compilers may fuse the
-- multiply-subtract in PUC 5.1/5.2 `%`, which changes their last bits)
local mt = {}
for _, e in ipairs({"add", "sub", "mul", "div", "mod", "pow"}) do
  mt["__" .. e] = function(a, b) return e end
end
local obj = setmetatable({}, mt)
local vals = {3, -7, 0, 2.5, -0.0, 1e15, -1, 64, "10", "0x10", "2.5", obj}
local ops = {
  function(a, b) return a + b end, function(a, b) return a - b end,
  function(a, b) return a * b end, function(a, b) return a / b end,
  function(a, b) return a % b end, function(a, b) return a ^ b end,
}
for i, a in ipairs(vals) do
  for j, b in ipairs(vals) do
    local row = {}
    for _, f in ipairs(ops) do
      local ok, r = pcall(f, a, b)
      if ok then
        r = tostring(r)
        if r:find("nan") then r = "nan" end
      else
        r = "E:" .. string.gsub(string.gsub(tostring(r), "^[^:]*:%d+: ", ""), " %(.*$", "")
      end
      row[#row + 1] = r
    end
    print(i, j, table.concat(row, "|"))
  end
end
local x, y = 17, -5
print(x % 3, x % -3, y % 3, y % -3, x + 0.5, x * 2, x - 2.5, 1 / x)
