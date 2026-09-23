-- v3.1 iopkg: the 5.1 os library. os.date walks its format as a C string
-- and passes a trailing '%' through; os.time takes any number (a string
-- too) for a field, treats anything else as absent, reads fields from the
-- seconds upwards and does not write the normalised fields back; difftime
-- truncates and defaults its second time to 0; os.execute returns the raw
-- wait status; os.rename names the source file; os.exit takes no boolean.
-- Times are compared as differences, so the time zone does not matter.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local t0 = 86400 * 45 + 3600 * 15 + 61
p("date", os.date("!%Y-%m-%d %H:%M:%S %j %a %b %p %I %U %W %y %C %D %e %T %R %r", t0))
p("date C string", os.date("!%Y\0%m", t0), os.date("!ab%", t0))
p("date number format", os.date(12, t0), os.date("!%Y", "86400"))
p("date bad time", pcall(os.date, "!%Y", {}))
local ref = os.time{year = 2000, month = 1, day = 1, hour = 0}
p("time string fields", os.time{year = "2000", month = "1", day = "2", hour = "0"} - ref)
p("time bool field", os.time{year = 2000, month = 1, day = 1, hour = true} - ref)
p("time float field", os.time{year = 2000, month = 1, day = 1.9, hour = 0} - ref)
p("time missing", pcall(os.time, {}))
p("time missing month", pcall(os.time, {year = 2000, day = 1}))
local nt = {year = 2000, month = 14, day = 1, hour = 0}
p("time no write back", os.time(nt) - ref, nt.month, nt.yday)
p("difftime", os.difftime(10), os.difftime(10.9, 3.9), os.difftime("10", "4"))
p("execute", os.execute("exit 3"), os.execute())
local base = os.tmpname()
p("tmpname", (base:gsub("[%w]+$", "X")), io.open(base) ~= nil)
os.remove(base)
local r = {os.rename(base, base .. "_to")}
p("rename missing", r[1], (tostring(r[2]):gsub(base:gsub("%p", "%%%0"), "TMP")), r[3])
r = {os.remove(base)}
p("remove missing", r[1], (tostring(r[2]):gsub(base:gsub("%p", "%%%0"), "TMP")), r[3])
p("exit bool", pcall(os.exit, true))
p("setlocale", os.setlocale(), os.setlocale("C"), pcall(os.setlocale, nil, "bogus"))
p("getenv number", os.getenv(1234567))
