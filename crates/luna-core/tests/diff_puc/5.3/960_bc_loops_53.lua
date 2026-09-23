-- v3.1 bytecode: generic loops with captured variables, integer and float
-- numeric loops, bitwise operators with constant operands, closing jumps.
local fns = {}
for k, v in pairs({one = 1}) do
  for i = 1, 2 do
    fns[#fns + 1] = function() return k, v, i end
  end
end
for _, f in ipairs(fns) do print(f()) end
for x = 1.5, 3 do io.write(x, " ") end
print()
local m = 0xF0
print(m & 0x3C, m | 3, m ~ 0xFF, m >> 4, 1 << 5, ~m, 7 // 2, 7.5 // 2)
local caps = {}
for n = 1, 5 do
  local c = n
  caps[#caps + 1] = function() return c end
  if n * 2 > 6 then break end
end
for _, f in ipairs(caps) do io.write(f(), " ") end
print()
