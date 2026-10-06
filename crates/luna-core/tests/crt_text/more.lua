-- More of the C library's stream behaviour: setvbuf sizes, what a failed
-- number read leaves, ungetc at the start of a buffer, and writing right
-- after reading. File names are relative to the global DIR when it is set.
io.stdout:setvbuf("no")
local D = DIR or ""
local function P(n) return D .. n end
local L = _VERSION >= "Lua 5.2"
local unpack = table.unpack or unpack
local function esc(s)
  if s == nil then return "nil" end
  if io.type(s) then return "file" end
  if type(s) ~= "string" then return tostring(s) end
  return (s:gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end))
end
local function raw(name) local f = assert(io.open(name, "rb")); local s = f:read("*a"); f:close(); return s end
local function put(name, mode, s) local f = assert(io.open(name, mode)); f:write(s); f:close() end
local function r(...) local t = table.pack and table.pack(...) or {n = select("#", ...), ...}
  local o = {} for i = 1, t.n do o[i] = esc(t[i]) end return table.concat(o, ",") end
-- (1) setvbuf, then reads and seeks
for _, data in ipairs({ {"lf", "abc\n"}, {"crlf", "abc\r\n"} }) do
  put(P("v.txt"), "wb", string.rep(data[2], 300))
  for _, sv in ipairs({ {"no"}, {"full", 100}, {"full", 5000}, {"line", 30}, {"full", 7}, {"full"}, {"full", 513} }) do
    for _, mode in ipairs({"r", "rb"}) do
      local f = assert(io.open(P("v.txt"), mode))
      local ok = f:setvbuf(sv[1], sv[2])
      local t = {}
      t[#t+1] = esc(f:read(1)) .. "@" .. r(f:seek("cur"))
      t[#t+1] = esc(f:read("*l")) .. "@" .. r(f:seek("cur"))
      t[#t+1] = #(f:read(600) or "") .. "@" .. r(f:seek("cur"))
      t[#t+1] = esc(f:read("*l")) .. "@" .. r(f:seek("cur"))
      t[#t+1] = #(f:read("*a") or "") .. "@" .. r(f:seek("cur"))
      f:close()
      print("setvbuf", data[1], sv[1], tostring(sv[2]), mode, tostring(ok), table.concat(t, "|"))
    end
  end
end
for _, sv in ipairs({ {"no"}, {"full", 100}, {"line", 30} }) do
  for _, mode in ipairs({"w", "wb"}) do
    local f = assert(io.open(P("w.txt"), mode)); f:setvbuf(sv[1], sv[2])
    local t = {}
    for i = 1, 3 do f:write("x\n"); t[#t+1] = r(f:seek("cur")) end
    f:write(string.rep("y\n", 60)); t[#t+1] = r(f:seek("cur"))
    f:close()
    print("setvbuf write", sv[1], tostring(sv[2]), mode, table.concat(t, "|"), #raw(P("w.txt")))
  end
end
-- (2) numbers: what *n takes and what is left
local nums = { "1e+x", "0x1p", "  12abc", "1..2", "-.e1", "0x", "inf", "nan", "1e", "12", ".5",
  "- 3", "0x1P-2z", "1e+", "--1", "0xg", "12345678901234567890", "1.5e", "+.", "0x.8", "\n\n7\r\n" }
for _, s in ipairs(nums) do
  for _, mode in ipairs({"r", "rb"}) do
    put(P("n.txt"), "wb", s)
    local f = assert(io.open(P("n.txt"), mode))
    local a = { f:read("*n") }
    local rest = f:read("*a")
    f:close()
    print("number", esc(s), mode, r(unpack(a, 1, 3)), esc(rest))
    f = assert(io.open(P("n.txt"), mode))
    local b1, b2 = f:read("*n", "*n")
    print("number2", esc(s), mode, r(b1, b2), esc(f:read("*a")))
    f:close()
  end
end
-- read(0) at buffer edges
put(P("z.txt"), "wb", "ab\r\ncd")
for _, mode in ipairs({"r", "rb"}) do
  local f = assert(io.open(P("z.txt"), mode))
  local t = {}
  for i = 1, 8 do t[#t+1] = esc(f:read(0)) .. esc(f:read(1)) end
  print("read0", mode, table.concat(t, "|")); f:close()
end
-- (5) read, then write without a seek
for _, mode in ipairs({"r+", "r+b", "a+", "a+b", "w+", "w+b"}) do
  put(P("rw.txt"), "wb", "aa\nbb\ncc\n")
  local f = assert(io.open(P("rw.txt"), mode))
  if mode:sub(1, 1) == "w" then f:write("aa\nbb\ncc\n"); f:seek("set", 0) end
  local t = {}
  t[#t+1] = esc(f:read("*l"))
  t[#t+1] = r(f:write("X\n"))
  t[#t+1] = r(f:flush())
  t[#t+1] = esc(f:read("*l"))
  t[#t+1] = r(f:seek("cur"))
  t[#t+1] = r(f:write("Y\n"))
  t[#t+1] = esc(f:read("*a"))
  t[#t+1] = r(f:write("Z\n"))
  f:close()
  print("rw", mode, table.concat(t, "|"), esc(raw(P("rw.txt"))))
  put(P("rw.txt"), "wb", "aa\nbb\n")
  f = assert(io.open(P("rw.txt"), mode))
  if mode:sub(1, 1) == "w" then f:write("aa\nbb\n"); f:seek("set", 0) end
  local a = esc(f:read("*a"))
  local w = r(f:write("E\n"))
  f:close()
  print("rw eof", mode, a, w, esc(raw(P("rw.txt"))))
end

-- ungetc at the start of a buffer: read(0) then *n right after a refill
for _, sv in ipairs({ {"full", 4}, {"full", 2}, {"no"} }) do
  for _, mode in ipairs({"r", "rb"}) do
    put(P("u.txt"), "wb", "12 34 56 78\n9")
    local f = assert(io.open(P("u.txt"), mode)); f:setvbuf(sv[1], sv[2])
    local t = {}
    for i = 1, 6 do t[#t+1] = esc(f:read(0)) .. ":" .. r(f:read("*n")) .. "@" .. r(f:seek("cur")) end
    t[#t+1] = esc(f:read("*a"))
    f:close()
    print("unget", sv[1], tostring(sv[2]), mode, table.concat(t, "|"))
  end
end
