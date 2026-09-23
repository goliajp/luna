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
-- 5.1 ltablib / luaB_unpack: raw access, C int positions, no bounds
-- errors, and the 5.1 quicksort's own bounds checks.
local function A(n) local t = {} for i = 1, n do t[i] = i * 10 end return t end
local function dump(t) local o = {} for k, v in pairs(t) do o[#o + 1] = k .. "=" .. tostring(v) end table.sort(o) return table.concat(o, ",") end
T("remove(A3,7)", table.remove, A(3), 7)
T("remove({})", table.remove, {})
T("remove(A3,0)", table.remove, A(3), 0)
do local t = A(3) table.insert(t, 6, "x") print("insert past end", dump(t)) end
do local t = A(3) table.insert(t, -1, "x") print("insert at -1", dump(t)) end
T("insert(A3,1,2,3)", table.insert, A(3), 1, 2, 3)
T("setn(1)", table.setn, 1)
T("setn({})", table.setn, {})
T("foreachi none", table.foreachi, A(3), function() end)
T("foreachi hit", table.foreachi, A(3), function(i, v) if v == 20 then return i, v end end)
T("foreach none", table.foreach, {a = 1}, function() end)
T("foreachi({},1)", table.foreachi, {}, 1)
T("getn(1)", table.getn, 1)
T("maxn", table.maxn, {[1.5] = 1, [-3] = 1, x = 1})
T("unpack 7997", function() return select("#", unpack({}, 1, 7997)) end)
T("unpack 7998", function() return select("#", unpack({}, 1, 7998)) end)
T("unpack 2^31", unpack, A(3), 1, 2^31)
T("concat(A3,{})", table.concat, A(3), {})
T("concat({},'',1,2)", table.concat, {}, "", 1, 2)
T("sort({2,1},3)", table.sort, {2, 1}, 3)
T("sort({},3)", table.sort, {}, 3)
local lt = function(a, b) return a < b end
local always = function() return true end
local le = function(a, b) return a <= b end
print("sort lt", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, lt))
print("sort always", sorted({4, 3, 2, 1}, always))
print("sort le", sorted({3, 1, 3, 1, 2, 2, 3}, le))
print("sort always 10", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, always))
