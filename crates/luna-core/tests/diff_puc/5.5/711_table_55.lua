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
-- 5.5 ltablib: table.create sizes, aux_getn in unpack, strings need no
-- __len for checktab.
T("create(-1)", table.create, -1)
T("create(0,-1)", table.create, 0, -1)
T("create(2^31)", table.create, 2^31)
T("create(1.5)", table.create, 1.5)
T("create()", table.create)
T("create overflow", function()
  local ok, e = pcall(table.create, 0, 2^31 - 1)
  return ok, e, (e:find("^[^:]*:%d+:") ~= nil)
end)
T("unpack(42)", table.unpack, 42)
T("unpack('ab')", table.unpack, "ab")
T("concat('ab')", table.concat, "ab")
T("insert('ab','x')", table.insert, "ab", "x")
local lt = function(a, b) return a < b end
local always = function() return true end
print("sort always 10", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, always))
