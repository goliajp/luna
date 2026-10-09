-- What 5.1's strtoul retry leaves in errno, and what each dialect gives back
-- (5.1-5.3 read errno through a write right after a read). File names are
-- under the global DIR when it is set.
local D = DIR or ""
local function P(n) return D .. n end
local function put(name, s) local f = assert(io.open(name, "wb")); f:write(s); f:close() end
put(P("ro.txt"), "aa\n")
local function set9() local f = io.open(P("ro.txt"), "r"); f:write("X"); f:close() end
local function readerrno()
  put(P("rw.txt"), "aa\nbb\n")
  local f = io.open(P("rw.txt"), "r+"); f:read("*l")
  local a, b, c = f:write("X"); f:close()
  return a and "ok" or tostring(c)
end
for _, s in ipairs({ "123456789x", "11111111111111111111x", "1e999x", "10x", "0x", "0xg", "0x10", "0x10x", "1x1", "12345678x", "1234567890x", "99999999999999999999x", "  1x", "1 x", "1.5x", "1e5x", "0X1P4", "1X", "-10x", "0xffffffffffffffffffx" }) do
  set9()
  local v = tonumber(s)
  local e = readerrno()
  set9()
  local ok, w = pcall(function() return s + 0 end)
  local e2 = readerrno()
  print(string.format("%-24s", s), tostring(v), e, ok and tostring(w) or "error", e2)
end
