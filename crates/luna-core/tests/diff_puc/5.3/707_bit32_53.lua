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
-- 5.3 LUA_COMPAT_BITLIB: operands via luaL_checkinteger (floats must be
-- integral), 64-bit trimmed to 32; rotations read the displacement first.
T("band(3.5)", bit32.band, 3.5)
T("band(3.0)", bit32.band, 3.0)
T("band(-1)", bit32.band, -1)
T("band(2^40+5)", bit32.band, 2^40 + 5)
T("bnot(mini)", bit32.bnot, math.mininteger)
T("lshift(1,31)", bit32.lshift, 1, 31)
T("lshift(1,2^32+1)", bit32.lshift, 1, 2^32 + 1)
T("rshift(2^33+8,2)", bit32.rshift, 2^33 + 8, 2)
T("arshift(2^32+2^31,1)", bit32.arshift, 2^32 + 2^31, 1)
T("lrotate(3.5,'x')", bit32.lrotate, 3.5, "x")
T("rrotate(1,-1)", bit32.rrotate, 1, -1)
T("extract(-1,31,2)", bit32.extract, -1, 31, 2)
T("extract(2^32+7,0,4)", bit32.extract, 2^32 + 7, 0, 4)
T("replace(-1,0,4,4.5)", bit32.replace, -1, 0, 4, 4.5)
T("replace(2^33+7,2^33+1,0,4)", bit32.replace, 2^33 + 7, 2^33 + 1, 0, 4)
print("loaded", package.loaded.bit32 == bit32, require("bit32") == bit32)
