-- v3.1 iopkg: 5.5 keeps 5.4's require results and message assembly.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
package.path = base .. "_?.lua;" .. base .. "_?/x.lua"
package.cpath = ""
p("missing", pcall(require, "no.such"))
p("searchpath", package.searchpath("a_b", "?.x", "_", "#"))
package.preload.pre = function(...) return select("#", ...), ... end
p("preload", require("pre"))
p("no searchers", pcall(function() local s = package.searchers; package.searchers = {}; local r = {pcall(require, "zz")}; package.searchers = s; return table.unpack(r) end))
os.remove(base)
