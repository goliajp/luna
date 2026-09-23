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
-- 5.4 ltablib: remove blames argument 2 and compares unsigned; insert
-- position check is unsigned; unpack has no table check.
local function A(n) local t = {} for i = 1, n do t[i] = i * 10 end return t end
T("remove(A3,5)", table.remove, A(3), 5)
T("remove({},mini)", table.remove, {}, math.mininteger)
T("remove(A3,0)", table.remove, A(3), 0)
T("insert(A3,mini,'x')", table.insert, A(3), math.mininteger, "x")
T("insert(A3,1,2,3)", table.insert, A(3), 1, 2, 3)
T("unpack(42)", table.unpack, 42)
T("unpack({},maxi-1,maxi)", table.unpack, {}, math.maxinteger - 1, math.maxinteger)
T("concat({},'',maxi,maxi)", table.concat, {}, "", math.maxinteger, math.maxinteger)
local big = setmetatable({}, {__len = function() return math.maxinteger end})
T("insert len maxi", function() table.insert(big, "v") return rawget(big, math.mininteger) end)
T("remove len -5", table.remove, setmetatable({}, {__len = function() return -5 end}))
local lt = function(a, b) return a < b end
local always = function() return true end
print("sort always", sorted({4, 3, 2, 1}, always))
print("sort always 10", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, always))
print("sort 20", sorted({12, 3, 19, 7, 1, 15, 8, 20, 11, 2, 17, 5, 14, 9, 18, 4, 13, 6, 16, 10}, lt))
