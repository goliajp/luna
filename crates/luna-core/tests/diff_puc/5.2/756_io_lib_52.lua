-- v3.1 iopkg: the 5.2 io library. The methods live in the metatable, whose
-- close is io.close itself (no argument: the default output); writes return
-- the file; read formats still need the '*' but gain '*L'; numbers are read
-- with fscanf; a line iterator takes up to 17 formats; seek offsets are
-- floats that must be integral; popen's close reports nil on failure.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub("'_G%.", "'"):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local mt = getmetatable(io.stdout)
p("mt shape", mt.__index == mt, rawget(mt, "__name"), rawget(mt, "__close"))
local f = io.open(base, "w")
p("write", f:write("x") == f, f:flush())
p("seek float", pcall(f.seek, f, "set", 1.5))
p("seek str", f:seek("set", "1"))
p("open mode", pcall(io.open, base, "rb+"))
io.output(base)
p("method close default", mt.close())
p("io.flush closed", pcall(io.flush))
io.output(io.stdout)
p("close std", io.stderr:close())
f:close()
local function content(s) local g = io.open(base, "w"); g:write(s); g:close(); return io.open(base) end
f = content("12 0x10 1e5x\nline\nlast")
p("read *n", f:read("*n"), f:read("*n"), f:read("*n"), f:read("*L"))
p("read l", pcall(f.read, f, "l"))
p("read *L", f:read("*L"))
f:close()
f = content("a\n")
p("lines 17", pcall(function(...) return type(f:lines(...)) end, "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l"))
p("lines 18", pcall(f.lines, f, "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l", "*l"))
p("lines bad fmt", pcall(f:lines("l")))
f:close()
p("lines missing", pcall(io.lines, base .. "_missing"))
p("input missing", pcall(io.input, base .. "_missing"))
p("popen close 3", io.popen("exit 3"):close())
p("popen close 0", io.popen("exit 0"):close())
os.remove(base)
