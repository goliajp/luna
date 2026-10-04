-- fmod with NaN operands. On x86 Linux, gcc builds PUC's fmod as an x87
-- fprem loop: with two NaNs it returns the one with the larger significand,
-- or the positive one when they differ only in sign, which print shows as
-- nan or -nan
local function nan(bits) return (string.unpack("<d", string.pack("<i8", bits))) end
local z = 0
local n = z / z
local p = -n
local small, big = nan(0x7FF8000000000003), -nan(0x7FF8000000000005)
local cases = { { n, p }, { p, n }, { n, n }, { p, p }, { n, 3.5 }, { 3.5, p },
  { 1.5, z + 0.0 }, { small, big }, { big, small } }
for i, c in ipairs(cases) do
  print(i, math.fmod(c[1], c[2]), c[1] % c[2], 1.0 % "0" % c[2])
end
