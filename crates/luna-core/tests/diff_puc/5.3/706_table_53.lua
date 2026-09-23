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
-- 5.3 ltablib: checktab duck typing, lua_geti/seti, lua_Integer
-- positions (remove still blames argument 1), no table check in unpack,
-- argument order of move, and the 5.3 quicksort.
local function A(n) local t = {} for i = 1, n do t[i] = i * 10 end return t end
T("remove(A3,5)", table.remove, A(3), 5)
T("remove({},mini)", table.remove, {}, math.mininteger)
T("remove({},0)", table.remove, {}, 0)
T("insert(A3,5,'x')", table.insert, A(3), 5, "x")
T("insert(A3,2.5,'x')", table.insert, A(3), 2.5, "x")
T("unpack(42)", table.unpack, 42)
T("unpack(42,1,2)", table.unpack, 42, 1, 2)
T("unpack('ab')", table.unpack, "ab")
T("unpack len 2.0", table.unpack, setmetatable({7, 8}, {__len = function() return 2.0 end}))
T("unpack len 2.5", table.unpack, setmetatable({}, {__len = function() return 2.5 end}))
T("unpack len '2'", table.unpack, setmetatable({7, 8}, {__len = function() return "2" end}))
T("move('x',1,2,3)", table.move, "x", 1, 2, 3)
T("move({},1,2,3,'x')", table.move, {}, 1, 2, 3, "x")
T("move({},1,'x',3)", table.move, {}, 1, "x", 3)
do
  local log = {}
  local mt = {__eq = function() log[#log + 1] = "eq" return true end}
  local a = setmetatable({1, 2, 3}, mt)
  local b = setmetatable({}, mt)
  local order = {}
  setmetatable(b, {__eq = mt.__eq, __newindex = function(t, k, v) order[#order + 1] = k rawset(t, k, v) end})
  table.move(a, 1, 3, 2, b)
  print("move __eq", table.concat(log, ","), table.concat(order, ","))
end
T("concat({{}})", table.concat, {{}})
T("concat(named)", table.concat, {setmetatable({}, {__name = "Thing"})})
T("concat(1,{})", table.concat, 1, {})
T("sort({},3)", table.sort, {}, 3)
T("sort({1},3)", table.sort, {1}, 3)
T("sort({2,1},3)", table.sort, {2, 1}, 3)
T("sort len 2^40", table.sort, setmetatable({}, {__len = function() return 2^40 end}))
local lt = function(a, b) return a < b end
local always = function() return true end
print("sort lt", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, lt))
print("sort always", sorted({4, 3, 2, 1}, always))
print("sort always 10", sorted({5, 9, 1, 7, 3, 8, 2, 10, 4, 6}, always))
local store = {3, 1, 2}
local proxy = setmetatable({}, {__len = function() return 3 end,
  __index = function(_, k) return store[k] end,
  __newindex = function(_, k, v) store[k] = v end})
table.sort(proxy)
print("sort proxy", table.concat(store, ","))
