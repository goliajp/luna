-- v3.1 iopkg: the 5.3 package library has no loaders, module or seeall;
-- searchpath still skips empty templates; require returns one value.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
p("shape", rawget(package, "loaders"), rawget(package, "seeall"), rawget(_G, "module"), package.loaded.bit32 == bit32)
p("config", package.config)
package.path = base .. "_?.lua;;" .. base .. "_?/x.lua"
package.cpath = ""
p("missing", pcall(require, "no.such"))
p("searchpath", package.searchpath("a.b", ";x_?;;y_?"))
local f = io.open(base .. "_m1.lua", "w"); f:write("return select('#', ...), ...") f:close()
p("require", require("m1"))
os.remove(base .. "_m1.lua")
os.remove(base)
