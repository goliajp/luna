-- v3.1 iopkg: the 5.3 io library. The metatable gains __name; read formats
-- lose the '*'; integers read as integers; a float count must be integral;
-- a non-string format is a type error; numbers are written with %.14g (no
-- ".0"); line iterators take up to 250 formats; popen checks its mode;
-- open accepts any number of 'b's; a closed default file is "standard".
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local mt = getmetatable(io.stdout)
p("mt shape", mt.__index == mt, rawget(mt, "__name"), rawget(mt, "__close"))
local f = io.open(base, "w")
f:write(1.0, " ", -0.0, " ", 2^63, " ", 7, " ", 1e100)
f:close()
f = io.open(base)
p("written numbers", f:read("a"))
f:close()
local function content(s) local g = io.open(base, "w"); g:write(s); g:close(); return io.open(base) end
f = content("12 0x10 1.5 0x\nnext")
p("read n", f:read("n", "n", "n", "n"))
p("read count float", pcall(f.read, f, 1.5))
p("read table", pcall(f.read, f, {}))
p("read x", pcall(f.read, f, "x"))
p("read L", f:read("L"), f:read("L"), f:read("l"))
f:close()
p("open modes", pcall(io.open, base, "rbb"), pcall(io.open, base, "rb+"))
p("popen mode", pcall(io.popen, "true", "rb"))
p("setvbuf size", pcall(f.setvbuf, io.stdout, "no", "x"))
io.input(base)
io.close(io.input())
p("io.read closed", pcall(io.read))
io.input(io.stdin)
p("close std", io.stdout:close())
p("closed io.input", pcall(io.input, f))
os.remove(base)
