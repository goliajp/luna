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
-- 5.2 lmathlib: float results, number bounds for random, log with a
-- base (log10 special case only), and no 5.3 integer API.
local z = 0
local negz = -(z * 1.0)
T("floor(-0)", math.floor, negz)
T("ceil(-0.25)", math.ceil, -0.25)
T("modf(-2)", math.modf, -2)
T("log(8,2)", math.log, 8, 2)
T("log(100,10)", math.log, 100, 10)
T("log(8,nil)", math.log, 8, nil)
T("deg(3.7)", math.deg, 3.7)
T("atan(1,2)", math.atan, 1, 2)
T("atan2(1,2)", math.atan2, 1, 2)
T("max(2,'10')", math.max, 2, "10")
T("min(1,{})", math.min, 1, {})
T("random(3.5) ok", function() return math.random(3.5) <= 4 end)
T("random(0.5)", math.random, 0.5)
T("random(2,1)", math.random, 2, 1)
T("random(2^53) ok", function() return math.random(2^53) >= 1 end)
T("randomseed(3.5)", math.randomseed, 3.5)
T("randomseed('x')", math.randomseed, "x")
T("ldexp(1,2^32+1)", math.ldexp, 1, 2^32 + 1)
T("ldexp(1,'x')", math.ldexp, 1, "x")
print("mod", math.mod, "pow", type(math.pow), "maxinteger", math.maxinteger, "ult", math.ult)
