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
-- 5.2 bit32: luaL_checkunsigned rounds a float to nearest (ties to even)
-- and wraps it mod 2^32; shift/field arguments are C ints; field errors
-- have their own wording; the module is in package.loaded.
T("band(3.5)", bit32.band, 3.5)
T("band(2.5)", bit32.band, 2.5)
T("band(-1)", bit32.band, -1)
T("band(-2.5)", bit32.band, -2.5)
T("bor(2^32+3)", bit32.bor, 2^32 + 3)
T("bxor('7',1)", bit32.bxor, "7", 1)
T("bnot(0.5)", bit32.bnot, 0.5)
T("btest(1.5,2)", bit32.btest, 1.5, 2)
T("lshift(1,2^32+3)", bit32.lshift, 1, 2^32 + 3)
T("rshift(-1,2.9)", bit32.rshift, -1, 2.9)
T("arshift(2^31,4)", bit32.arshift, 2^31, 4)
T("lrotate()", bit32.lrotate)
T("rrotate(1)", bit32.rrotate, 1)
T("extract(-1,-1)", bit32.extract, -1, -1)
T("extract(-1,0,0)", bit32.extract, -1, 0, 0)
T("extract(-1,30,3)", bit32.extract, -1, 30, 3)
T("extract(0xF0,4,4)", bit32.extract, 0xF0, 4, 4)
T("replace(0,1,31)", bit32.replace, 0, 1, 31)
T("replace(0,-1,0,32)", bit32.replace, 0, -1, 0, 32)
T("band('x')", bit32.band, "x")
print("loaded", package.loaded.bit32 == bit32, require("bit32") == bit32)
