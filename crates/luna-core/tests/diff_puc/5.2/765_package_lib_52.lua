-- v3.1 iopkg: the 5.2 package library. searchers arrive with loaders as an
-- alias, and searchpath; preload lives in the registry; the loader gets
-- the name and the searcher's extra value; require returns one value;
-- module returns the module table and skips options that are not
-- functions; bit32 is among the loaded libraries.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub("'_G%.", "'"):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local made = {}
local function mod(name, src) local f = io.open(base .. "_" .. name .. ".lua", "w"); f:write(src); f:close(); made[#made + 1] = base .. "_" .. name .. ".lua" end
p("shape", package.loaders == package.searchers, type(package.searchpath), type(package.seeall), type(module))
p("registry", package.preload == debug.getregistry()._PRELOAD, package.loaded.bit32 == bit32)
package.path = base .. "_?.lua;;" .. base .. "_?/x.lua"
package.cpath = base .. "_?.so"
p("missing", pcall(require, "no.such"))
p("searchpath", package.searchpath("a.b", ";x_?;;y_?"))
mod("m1", "return select('#', ...), ...")
p("require", require("m1"))
package.preload.pre = function(...) return select("#", ...) end
p("preload", require("pre"))
local pk = package
package.searchers = 3
p("bad searchers", pcall(require, "zz"))
package.searchers = pk.loaders
mod("mm", "local r = module(..., 5, package.seeall); x = r == _M; function get() return type(print) end")
local m = require("mm")
p("module", m._NAME, m._PACKAGE, m.x, m.get())
p("module from C", pcall(module, "cm"))
for _, f in ipairs(made) do os.remove(f) end
os.remove(base)
