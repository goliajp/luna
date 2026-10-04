-- 5.1/5.2 `%` is a - floor(a/b)*b. On aarch64 the C compiler fuses the
-- multiply and the subtraction into one instruction with one rounding, so
-- these differ from the two-step result in the last bits or more
local function show(x) return string.format("%.17g", x) end
local pairs_ = {
  { 229370661.88518453, -1706.8768649859712 },
  { 2550748408.5628519, 184.51454242901622 },
  { 4224018212046483, -37.716350000219585 },
  { 4231682335600.1089, 14.375423530291497 },
  { -4.9994521541518397e+18, -0.021076274463476742 },
}
for i, p in ipairs(pairs_) do
  local a, b = p[1], p[2]
  print(i, show(a % b))
end
-- operands known at compile time are folded by the compiler
print(show(229370661.88518453 % -1706.8768649859712))
print(show(4231682335600.1089 % 14.375423530291497))
print(show(4224018212046483 % -37.716350000219585))
-- tonumber with a base accumulates n * base + digit, fused the same way
print(show(tonumber("j8kj9g93d1f69i58e6a42g878179hl", 22)))
print(show(tonumber("5p2qaa8qi1htmec6ogfetnl9ks2b", 31)))
print(show(tonumber("30a16ga0fe6bf19gdd47cd516", 18)))
