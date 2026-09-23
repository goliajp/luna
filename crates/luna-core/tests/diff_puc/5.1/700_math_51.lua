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
    log[#log + 1] = a .. b
    return cmp(a, b)
  end)
  local out = {}
  for i = 1, #arr do out[i] = tostring(t[i]) end
  return tostring(ok) .. " " .. (ok and "" or clean(e)) .. " [" .. table.concat(out, ",") .. "] " .. table.concat(log, " ")
end
-- 5.1 lmathlib: every result is a double and integer arguments go
-- through luaL_checkint, a C int.
local z = 0
local negz = -(z * 1.0)
T("floor(-0)", math.floor, negz)
T("ceil(-0.5)", math.ceil, -0.5)
T("modf(-0)", math.modf, negz)
T("modf(-inf)", math.modf, -1 / (z * 1.0))
T("modf(-3)", math.modf, -3)
T("fmod(-4,2)", math.fmod, -4, 2)
T("deg(3.7)", math.deg, 3.7)
T("log(8,2)", math.log, 8, 2)
T("atan(1,2)", math.atan, 1, 2)
T("max('10','9')", math.max, "10", "9")
T("max()", math.max)
T("min(1,'x')", math.min, 1, "x")
T("ldexp(1,2^32+1)", math.ldexp, 1, 2^32 + 1)
T("random(0)", math.random, 0)
T("random(2^31)", math.random, 2^31)
T("random(3,1)", math.random, 3, 1)
T("random(1,2,3)", math.random, 1, 2, 3)
T("randomseed(7)", math.randomseed, 7)
T("randomseed()", math.randomseed)
print("mod is fmod", math.mod == math.fmod)
print("maxinteger", math.maxinteger, "mininteger", math.mininteger, "type", math.type)
