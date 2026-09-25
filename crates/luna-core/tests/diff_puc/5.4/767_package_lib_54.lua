-- v3.1 iopkg: 5.4's require also returns the loader data (":preload:" or
-- the file name), and its messages are assembled differently: searchers
-- return their text without the "\n\t" lead and searchpath lists empty
-- templates too.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
package.path = base .. "_?.lua;;" .. base .. "_?/x.lua"
package.cpath = base .. "_?.so"
p("missing", pcall(require, "no.such"))
p("searchpath", package.searchpath("a.b", ";x_?;;y_?"))
p("searcher preload miss", package.searchers[1]("nope"))
local f = io.open(base .. "_m1.lua", "w"); f:write("return select('#', ...), ...") f:close()
p("require", require("m1"))
p("require again", require("m1"))
package.preload.pre = function(...) return ... end
p("preload", require("pre"))
table.insert(package.searchers, 1, function() return nil end)
table.insert(package.searchers, 1, function(n) return "custom " .. n end)
p("custom", pcall(require, "zz"))
os.remove(base .. "_m1.lua")
os.remove(base)
