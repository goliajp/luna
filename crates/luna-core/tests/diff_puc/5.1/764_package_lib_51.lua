-- v3.1 iopkg: the 5.1 package library. package has loaders (no searchers
-- or searchpath) and a config without the final newline; preload lives in
-- the package table; require runs the loaders from the original package
-- table, hands the loader only the name, returns one value, and marks a
-- module being loaded so a loop or an earlier failure is reported; module
-- returns nothing and calls every option.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local made = {}
local function mod(name, src) local f = io.open(base .. "_" .. name .. ".lua", "w"); f:write(src); f:close(); made[#made + 1] = base .. "_" .. name .. ".lua" end
local keys = {}
for k, v in pairs(package) do if k ~= "loaded" then keys[#keys + 1] = k .. ":" .. type(v) end end
table.sort(keys)
p("keys", table.concat(keys, ","))
p("config", package.config)
p("preload in registry", package.preload == debug.getregistry()._PRELOAD, package.loaded == debug.getregistry()._LOADED)
package.path = base .. "_?.lua;;" .. base .. "_?/x.lua"
package.cpath = base .. "_?.so"
p("missing", pcall(require, "no.such"))
mod("m1", "return select('#', ...), ...")
p("require", require("m1"))
mod("m2", "error('boom')")
p("error", pcall(require, "m2"))
p("error again", pcall(require, "m2"))
mod("m3", "return require('m3')")
p("loop", pcall(require, "m3"))
package.preload.pre = function(...) return select("#", ...) end
p("preload", require("pre"))
local saved = package.loaders
package.loaders = nil
p("no loaders", pcall(require, "m4"))
package.loaders = saved
local pk = package
package = {}
p("package replaced", require("m1"))
package = pk
mod("mm", "local n = select('#', module(..., package.seeall)); x = n; function get() return type(print) end")
local m = require("mm")
p("module", m._NAME, m._PACKAGE, m._M == m, m.x, m.get())
p("module from C", pcall(module, "cm"))
p("module option", pcall(loadstring("module('opt', function(t) t.opted = true end); return opted")))
for _, f in ipairs(made) do os.remove(f) end
os.remove(base)
