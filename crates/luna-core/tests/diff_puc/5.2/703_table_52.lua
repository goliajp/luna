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
-- 5.2 ltablib: raw access with __len honoured and truncated to a C int,
-- positions checked (remove blames argument 1), unpack also global.
local function A(n) local t = {} for i = 1, n do t[i] = i * 10 end return t end
T("remove(A3,5)", table.remove, A(3), 5)
T("remove(A3,4)", table.remove, A(3), 4)
T("remove({},0)", table.remove, {}, 0)
T("remove({},-1)", table.remove, {}, -1)
T("insert(A3,5,'x')", table.insert, A(3), 5, "x")
T("insert(A3,0,'x')", table.insert, A(3), 0, "x")
T("insert(A3,2.9,'x')", function() local t = A(3) table.insert(t, 2.9, "x") return t[2], t[3] end)
local len27 = setmetatable({}, {__len = function() return 2.7 end})
T("__len 2.7 insert", function() table.insert(len27, "v") return rawget(len27, 3) end)
T("__len '2' unpack", table.unpack, setmetatable({5, 6, 7}, {__len = function() return "2" end}))
T("__len nil", table.insert, setmetatable({}, {__len = function() end}), "v")
T("__len 2^32+1", function() return select("#", table.unpack(setmetatable({}, {__len = function() return 2^32 + 1 end}))) end)
print("unpack is table.unpack", unpack == table.unpack)
T("maxn", table.maxn, {[2.5] = 1, [-7] = 1, [1] = 1})
T("maxn(1)", table.maxn, 1)
T("concat(3)", table.concat, 3, {})
T("concat sep first", table.concat, 3, {})
T("unpack({},1,2^31)", table.unpack, {}, 1, 2^31)
T("sort({},3)", table.sort, {}, 3)
local lt = function(a, b) return a < b end
local always = function() return true end
print("sort lt", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, lt))
print("sort always", sorted({4, 3, 2, 1}, always))
print("sort always 10", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, always))
