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
-- 5.4 math.random: xoshiro256** projected into [low, up] by masking to
-- the next Mersenne number and retrying (PUC 'project'); integer seeds.
local function seq(label, s1, s2, ...)
  math.randomseed(s1, s2)
  local out = {}
  for i = 1, 10 do out[i] = tostring(math.random(...)) end
  print(label, table.concat(out, " "))
end
seq("(100)", 42, 0, 100)
seq("(1,6)", 42, 0, 1, 6)
seq("(-10,10)", 7, 9, -10, 10)
seq("(3)", -1, -1, 3)
seq("(1000)", 2^62, 5, 1000)
seq("(0)", 1, 0, 0)
seq("()", 3, 0)
print("seeds", math.randomseed(123, 456))
print("seed1", math.randomseed(-5))
T("randomseed(3.5)", math.randomseed, 3.5)
T("randomseed(nil)", math.randomseed, nil)
T("random(2,1)", math.random, 2, 1)
T("random(1,2,3)", math.random, 1, 2, 3)
T("atan2(1)", math.atan2, 1)
T("floor(-0.0)", math.floor, -(0 * 1.0))
T("tointeger('8')", math.tointeger, "8")
print("atan2 is atan", math.atan2 == math.atan)
