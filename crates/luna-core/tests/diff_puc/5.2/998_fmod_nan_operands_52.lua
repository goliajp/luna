-- fmod with NaN operands. On x86 Linux, gcc builds PUC's fmod as an x87
-- fprem loop: with two NaNs it returns the positive one when they differ
-- only in sign, which print shows as nan or -nan
local z = 0
local n = z / z
local p = -n
local cases = { { n, p }, { p, n }, { n, n }, { p, p }, { n, 3.5 }, { 3.5, p }, { 1.5, z } }
for i, c in ipairs(cases) do
  print(i, math.fmod(c[1], c[2]), c[1] % c[2])
end
