-- tonumber follows each dialect's lbaselib: 5.1 converts a based numeral
-- with strtoul (sign, 0x prefix, clamping) into a double, 5.2 accumulates
-- a double, 5.3+ a wrapping integer; the base and the numeral are read in
-- the dialect's order.
-- Error messages are compared without their position ("@" marks one)
-- and without 5.2's hash-order-dependent "_G." qualifier.
local function clean(s)
  s = tostring(s):gsub("'_G%.", "'")
  local pos = s:match("^[^:\n]+:%d+: ") and "@" or ""
  return pos .. s:gsub("^[^:\n]+:%d+: ", "")
end
local function render(...)
  local out = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    local t = type(v)
    if t == "string" then out[#out + 1] = clean(v)
    elseif t == "number" or t == "boolean" or t == "nil" then out[#out + 1] = tostring(v)
    else out[#out + 1] = "<" .. t .. ">" end
  end
  return table.concat(out, " | ")
end
local function show(label, ...) print(label, render(pcall(...))) end
local function num(label, ...)
  local ok, v = pcall(tonumber, ...)
  if ok and type(v) == "number" and math.type then v = math.type(v) .. " " .. v end
  print(label, ok, clean(v))
end
num("ff 16", "ff", 16)
num("-ff 16", "-ff", 16)
num("  7\v 8", "  7\v", 8)
num("\v7 8", "\v7", 8)
num("0x10 16", "0x10", 16)
num("zz 36", "zz", 36)
num("1.5 10", "1.5", 10)
num("1e1 10", "1e1", 10)
num("big 10", "99999999999999999999", 10)
num("big 16", "ffffffffffffffffff", 16)
num("8 8", "8", 8)
num("empty 10", "", 10)
num("sign only", "-", 10)
num("number 17 16", 17, 16)
num("number 1.5 16", 1.5, 16)
num("base '16'", "ff", "16")
num("base 2.9", "1", 2.9)
num("base 1", "1", 1)
num("base 37", "1", 37)
num("table 99", {}, 99)
num("nil 99", nil, 99)
num("nil 16", nil, 16)
num("std 0x10", "0x10")
num("std 1e1", "1e1")
num("std table", {})
num("std nil", nil)
