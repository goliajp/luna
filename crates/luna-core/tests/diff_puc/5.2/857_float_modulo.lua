-- Float `%` per dialect: 5.1/5.2 compute a - floor(a/b)*b, 5.3 fixes
-- fmod's sign when m*b < 0 (which underflows for tiny operands), 5.4+
-- compare the signs.
local big, tiny = 1 / 0, 5e-324
local cases = {
  {5.5, -2}, {-5.5, 2}, {5, big}, {-5, big}, {5, -big}, {-5, -big},
  {1e308, 1e-308}, {tiny, -1e-300}, {-tiny, 1e-300}, {0.5, 0}, {-0.5, 0},
}
for _, c in ipairs(cases) do
  local r = c[1] % c[2]
  print(c[1], c[2], r ~= r and "nan" or string.format("%.17g", r))
end
