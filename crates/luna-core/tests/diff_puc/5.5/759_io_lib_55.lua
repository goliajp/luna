-- v3.1 iopkg: the 5.5 io library writes numbers the way tostring renders
-- them (so 1.0 is "1.0"), and a failed write returns the byte count as a
-- fourth value.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local f = io.open(base, "w")
f:write(1.0, " ", -0.0, " ", 1/3, " ", 7)
f:close()
f = io.open(base)
p("written numbers", f:read("a"))
p("write read-only", f:write("abc"))
f:close()
p("open missing", io.open(base .. "_missing"))
p("remove missing", os.remove(base .. "_missing"))
p("rename missing", os.rename(base .. "_missing", base .. "_x"))
os.remove(base)
