-- Reads and writes of files in and out of text mode, printed escaped; file
-- names are relative to the global DIR (a directory path ending in a
-- separator) when it is set.
local V = _VERSION
local L = V >= "Lua 5.2"
local D = DIR or ""
local function P(n) return D .. n end
local function esc(s)
  if s == nil then return "nil" end
  if type(s) ~= "string" then return tostring(s) end
  return (s:gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end))
end
local function raw(name)
  local f = assert(io.open(name, "rb")); local s = f:read("*a"); f:close(); return s
end
local function put(name, mode, s)
  local f = assert(io.open(name, mode)); f:write(s); f:close()
end
local DATA = "x\ny\r\nz\r\r\nw\rv\n\r\nend"
-- writes in text and binary mode, seen raw
put(P("t.txt"), "w", DATA); print("w raw", esc(raw(P("t.txt"))))
put(P("b.txt"), "wb", DATA); print("wb raw", esc(raw(P("b.txt"))))
put(P("t.txt"), "a", "app\n"); print("a raw", esc(raw(P("t.txt"))))
put(P("b.txt"), "ab", "app\n"); print("ab raw", esc(raw(P("b.txt"))))
local f = assert(io.open(P("rp.txt"), "w+")); f:write("1\n2\n"); f:seek("set", 0)
print("w+ read", esc(f:read("*a"))); print("w+ seek end", f:seek("end")); f:close()
print("w+ raw", esc(raw(P("rp.txt"))))
-- reads of a CRLF file in each mode
for _, mode in ipairs({"r", "rb"}) do
  local function o() return assert(io.open(P("b.txt"), mode)) end
  local h = o(); print(mode, "*a", esc(h:read("*a"))); h:close()
  h = o(); local t = {}
  while true do local l = h:read("*l"); if not l then break end; t[#t+1] = esc(l) end
  print(mode, "*l", table.concat(t, "|")); h:close()
  if L then
    h = o(); t = {}
    while true do local l = h:read("*L"); if not l then break end; t[#t+1] = esc(l) end
    print(mode, "L", table.concat(t, "|")); h:close()
  end
  h = o(); t = {}
  for n = 1, 6 do t[#t+1] = esc(h:read(n)) .. "@" .. tostring(h:seek("cur")) end
  print(mode, "count", table.concat(t, "|")); h:close()
  t = {}
  for l in io.lines(P("b.txt")) do t[#t+1] = esc(l) end
  print(mode, "io.lines", table.concat(t, "|"))
  h = o(); t = {}
  for l in h:lines() do t[#t+1] = esc(l) end
  print(mode, "f:lines", table.concat(t, "|")); h:close()
  h = o(); h:seek("set", 3); print(mode, "seek 3", esc(h:read("*a"))); h:close()
  h = o(); h:read("*l"); h:read("*l"); print(mode, "pos after 2 lines", h:seek("cur")); h:close()
end
-- numbers across CRLF
put(P("n.txt"), "wb", "12\r\n3.5\r\n-7\r\n")
for _, mode in ipairs({"r", "rb"}) do
  local h = assert(io.open(P("n.txt"), mode))
  print(mode, "*n", h:read("*n"), h:read("*n"), h:read("*n"), esc(h:read("*a"))); h:close()
end
-- Ctrl-Z
put(P("z.txt"), "wb", "ab\26cd\r\nef")
for _, mode in ipairs({"r", "rb"}) do
  local h = assert(io.open(P("z.txt"), mode)); print(mode, "ctrl-z *a", esc(h:read("*a"))); h:close()
  h = assert(io.open(P("z.txt"), mode)); print(mode, "ctrl-z *l", esc(h:read("*l")), esc(h:read("*l")), esc(h:read("*l"))); h:close()
end
put(P("z.txt"), "a", "more\n"); print("a after ctrl-z raw", esc(raw(P("z.txt"))))
put(P("z2.txt"), "wb", "ab\26"); put(P("z2.txt"), "a+", "c\n"); print("a+ after trailing ctrl-z raw", esc(raw(P("z2.txt"))))
-- a lone CR at the end, and a CR at a buffer boundary
put(P("e.txt"), "wb", "abc\r")
local h = assert(io.open(P("e.txt"), "r")); print("r", "trailing cr", esc(h:read("*a"))); h:close()
for _, k in ipairs({511, 512, 4095, 4096, 8191, 8192}) do
  put(P("big.txt"), "wb", string.rep("a", k) .. "\r\nb\r\n")
  h = assert(io.open(P("big.txt"), "r")); local s = h:read("*a"); h:close()
  h = assert(io.open(P("big.txt"), "r")); local l1 = h:read("*l"); local l2 = h:read("*l"); h:close()
  print("boundary", k, #s, esc(s:sub(-5)), #l1, esc(l2))
end
-- r+ : read then write
put(P("rw.txt"), "wb", "aa\r\nbb\r\n")
h = assert(io.open(P("rw.txt"), "r+")); print("r+ l", esc(h:read("*l"))); h:seek("cur", 0); h:write("X\n"); h:close()
print("r+ raw", esc(raw(P("rw.txt"))))
-- tmpfile
h = io.tmpfile(); h:write("t\n"); h:seek("set", 0); print("tmpfile", esc(h:read("*a")), h:seek("end")); h:close()
-- io.output / io.input with a name
io.output(P("o.txt")); io.write("o1\n"); io.close(); io.output(io.stdout)
print("io.output raw", esc(raw(P("o.txt"))))
io.input(P("b.txt")); print("io.input", esc(io.read("*l"))); io.close(); io.input(io.stdin)
-- ftell after the stream buffer was refilled, read past or bypassed
for _, f in ipairs({ {"lf", "abc\n"}, {"crlf", "abc\r\n"} }) do
  put(P("big2.txt"), "wb", string.rep(f[2], 3000))
  local h = assert(io.open(P("big2.txt"), "r"))
  local t = {}
  t[#t+1] = #h:read(5000) .. "@" .. tostring(h:seek("cur"))
  t[#t+1] = esc(h:read("*l")) .. "@" .. tostring(h:seek("cur"))
  t[#t+1] = esc(h:read(3)) .. "@" .. tostring(h:seek("cur"))
  h:seek("set", 100)
  t[#t+1] = esc(h:read(10)) .. "@" .. tostring(h:seek("cur"))
  t[#t+1] = esc(h:read("*l")) .. "@" .. tostring(h:seek("cur"))
  t[#t+1] = #h:read("*a") .. "@" .. tostring(h:seek("cur"))
  h:close()
  h = assert(io.open(P("big2.txt"), "r"))
  for _ = 1, 700 do h:read("*l") end
  t[#t+1] = "700 lines@" .. tostring(h:seek("cur"))
  t[#t+1] = esc(h:read(2)) .. "@" .. tostring(h:seek("end"))
  h:close()
  print("ftell", f[1], table.concat(t, "|"))
end
put(P("za.txt"), "wb", "ab\26"); put(P("za.txt"), "a", "c\n"); print("a after trailing ctrl-z raw", esc(raw(P("za.txt"))))
-- popen
local p = io.popen("echo hi")
if p then print("popen r", esc(p:read("*a"))); p:close() end
-- loadfile of a CRLF source with a ctrl-z
put(P("s.lua"), "wb", "return 'a\\\r\nb', [[x\r\ny]]\26 garbage")
local fn, err = loadfile(P("s.lua"))
if fn then print("loadfile", esc((fn())), esc(select(2, fn()))) else print("loadfile err", esc(err)) end
