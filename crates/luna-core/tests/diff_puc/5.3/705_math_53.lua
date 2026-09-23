local function clean(s)
  return (tostring(s):gsub("^[^:\n]*:%d+: ", ""):gsub("'_G%.", "'"))
end
local function fmt(v)
  if type(v) == "number" then
    -- the sign of a NaN is printed by the host libc (glibc "-nan", Apple
    -- "nan", MSVC "-nan(ind)"), so PUC's own output differs by platform
    local s = v ~= v and "nan" or string.format("%.17g", v)
    return s .. (math.type and (":" .. math.type(v)) or "")
  end
  return clean(v)
end
local function pack(...) return {n = select("#", ...), ...} end
local function T(label, f, ...)
  local r = pack(pcall(f, ...))
  local out = {label, "#" .. (r.n - 1)}
  for i = 1, r.n do out[#out + 1] = fmt(r[i]) end
  print(table.concat(out, " "))
end
-- comparator that logs every call, to pin the order PUC's quicksort uses
local function sorted(arr, cmp)
  local log, t = {}, {}
  for i = 1, #arr do t[i] = arr[i] end
  local ok, e = pcall(table.sort, t, cmp and function(a, b)
    log[#log + 1] = tostring(a) .. tostring(b)
    return cmp(a, b)
  end)
  local out = {}
  for i = 1, #arr do out[i] = tostring(t[i]) end
  return tostring(ok) .. " " .. (ok and "" or clean(e)) .. " [" .. table.concat(out, ",") .. "] " .. table.concat(log, " ")
end
-- 5.3 lmathlib: integer subtype, C-rand-based random with both
-- emptiness checks on argument 1, compat functions sharing math_atan.
T("random(0)", math.random, 0)
T("random(2,1)", math.random, 2, 1)
T("random(mini,0)", math.random, math.mininteger, 0)
T("random(mini,-1) ok", function() return math.type(math.random(math.mininteger, -1)) end)
T("random(3.5)", math.random, 3.5)
T("randomseed(3.5)", math.randomseed, 3.5)
T("randomseed()", math.randomseed)
T("tointeger('10')", math.tointeger, "10")
T("tointeger('0x10')", math.tointeger, "0x10")
T("tointeger('3.0')", math.tointeger, "3.0")
T("tointeger(3.5)", math.tointeger, 3.5)
T("tointeger()", math.tointeger)
T("type()", math.type)
T("max('10','9')", math.max, "10", "9")
T("min({})", function() return type(math.min({})) end)
T("max(1,{})", math.max, 1, {})
T("max(1,2.0)", math.max, 1, 2.0)
T("max(2,2.0)", math.max, 2, 2.0)
T("abs('-3')", math.abs, "-3")
T("floor('2.5')", math.floor, "2.5")
T("fmod(mini,-1)", math.fmod, math.mininteger, -1)
T("fmod(7,0)", math.fmod, 7, 0)
T("fmod(7,0.0)", math.fmod, 7, 0.0)
T("fmod('7',0)", math.fmod, "7", 0)
T("deg(3.7)", math.deg, 3.7)
T("atan2(1)", math.atan2, 1)
T("ldexp(1,2^32+1)", math.ldexp, 1, 2^32 + 1)
T("ldexp(1,2.5)", math.ldexp, 1, 2.5)
T("frexp(-0.5)", math.frexp, -0.5)
T("pow(2,0.5)", math.pow, 2, 0.5)
T("log(8,2)", math.log, 8, 2)
print("atan2 is atan", math.atan2 == math.atan, "cosh", type(math.cosh), "log10", type(math.log10))
