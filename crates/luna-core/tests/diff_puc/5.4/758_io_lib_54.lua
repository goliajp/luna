-- v3.1 iopkg: the 5.4 io library. The methods move out of the metatable
-- and __close appears; flush reports true (the file is only returned by
-- write); a closed default file is "default"; io.lines returns the file as
-- a fourth value. A file nobody closed is still flushed when collected,
-- because file handles are registered for finalization.
local base = os.tmpname()
local esc = base:gsub("%p", "%%%0")
local function clean(s) return (tostring(s):gsub(esc, "TMP"):gsub("0x%x+", "ADDR"):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local mt = getmetatable(io.stdout)
local keys = {}
for k in pairs(mt) do keys[#keys + 1] = k end
table.sort(keys)
p("mt keys", table.concat(keys, ","), mt.__index == mt)
do
  local f = io.open(base, "w")
  f:write("buffered")
end
collectgarbage()
collectgarbage()
local f = io.open(base)
p("collected file flushed", f:read("a"))
p("flush", f:flush(), io.stdout:flush())
p("seek set", f:seek("set", 2), f:read(2))
p("seek bad", pcall(f.seek, f, "sideways"))
p("setvbuf none", pcall(f.setvbuf, f))
f:close()
p("io.lines results", select("#", io.lines(base)))
io.output(base)
io.close()
p("io.flush closed", pcall(io.flush))
p("io.close closed", pcall(io.close))
io.output(io.stdout)
p("mt gc none", pcall(mt.__gc))
p("mt tostring table", pcall(mt.__tostring, {}))
p("popen close", io.popen("exit 3"):close())
os.remove(base)
