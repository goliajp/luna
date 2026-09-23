-- v3.1 iopkg: the 5.1 io library. The FILE* metatable holds the methods
-- itself and has no __name; writes report true, not the file; a standard
-- file refuses to close with nil; read formats must start with '*' and
-- have no 'L'; numbers are read with fscanf("%lf"), which converts the
-- numeral and leaves what follows it; line iterators take no
-- formats; errors carry C's strerror text. The file prefix is printed as TMP.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local mt = getmetatable(io.stdout)
local keys = {}
for k in pairs(mt) do keys[#keys + 1] = k end
table.sort(keys)
p("mt keys", table.concat(keys, ","), mt.__index == mt)
local f = io.open(base, "w")
p("write", f:write("x"), f:write(1.5, " ", 2^53))
p("flush", f:flush())
p("setvbuf bad", pcall(f.setvbuf, f, "bogus"))
p("seek negative", f:seek("set", -1))
p("close", f:close())
p("closed use", pcall(f.read, f))
p("close std", io.stdout:close())
p("method close none", pcall(mt.close))
p("mt gc none", pcall(mt.__gc))
p("io.type none", pcall(io.type))
p("io.type proxy", io.type(newproxy()))
local function content(s) local g = io.open(base, "w"); g:write(s); g:close(); return io.open(base) end
f = content("1.5x\n  0x10 -7e1\nline\nlast")
p("read *n", f:read("*n"), f:read("*l"))
p("read hex", f:read("*n"), f:read("*n"), f:read("*l"))
p("read l", pcall(f.read, f, "l"))
p("read *L", pcall(f.read, f, "*L"))
p("read *x", pcall(f.read, f, "*x"))
p("read table", pcall(f.read, f, {}))
p("read *a", f:read("*a"), f:read("*a"), f:read("*l"))
f:close()
f = content("a\nb\n")
local it = f:lines("*n")
p("lines ignores formats", it(), it(), it())
f:close()
p("lines missing", pcall(io.lines, base .. "_missing"))
p("open missing", io.open(base .. "_missing"))
p("open bad mode", io.open(base, "+r"))
p("open number", io.open(12345))
io.output(base)
io.close()
p("io.flush closed", pcall(io.flush))
io.output(io.stdout)
local pp = io.popen("exit 3")
p("popen close", pp:close())
os.remove(base)
