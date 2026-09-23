local function clean(s)
  return (tostring(s):gsub("^[^:\n]*:%d+: ", ""):gsub("'_G%.", "'"))
end
local function fmt(v)
  if type(v) == "number" then
    return string.format("%.17g", v) .. (math.type and (":" .. math.type(v)) or "")
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
-- 5.5 math: no LUA_COMPAT_MATHLIB (so no atan2/cosh/pow/log10), frexp
-- and ldexp are core again; ldexp's exponent is a C int.
print("atan2", math.atan2, "cosh", math.cosh, "pow", math.pow, "log10", math.log10)
T("ldexp(1,2^32+1)", math.ldexp, 1, 2^32 + 1)
T("ldexp(1,'x')", math.ldexp, 1, "x")
T("frexp()", math.frexp)
T("frexp(12)", math.frexp, 12)
local function seq(label, s1, s2, ...)
  math.randomseed(s1, s2)
  local out = {}
  for i = 1, 10 do out[i] = tostring(math.random(...)) end
  print(label, table.concat(out, " "))
end
seq("(100)", 42, 0, 100)
seq("(mini,maxi)", 1, 0, math.mininteger, math.maxinteger)
T("random(0) type", function() return math.type(math.random(0)) end)
T("max()", math.max)
T("max(nan,1)", math.max, 0/0, 1)
T("min(0.0,-0.0)", math.min, 0.0, -(0 * 1.0))
