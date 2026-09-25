-- v3.1 iopkg: the 5.4 os library reads the date fields from the year down
-- and bounds each after removing its offset; an unrepresentable date is a
-- "date result" error.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local ref = os.time{year = 2000, month = 1, day = 1, hour = 0}
p("time missing", pcall(os.time, {}))
p("time year bound", pcall(os.time, {year = 2147483648 + 1900, month = 1, day = 1}))
p("time day bound", pcall(os.time, {year = 2000, month = 1, day = 2147483648}))
p("time month big", os.time{year = 2000, month = 1 + 12 * 1000, day = 1, hour = 0} - ref)
p("time not integer", pcall(os.time, {year = 2000, month = 1.5, day = 1}))
p("date unrepresentable", pcall(os.date, "!%Y", 2^62))
p("date *t", (function() local t = os.date("!*t", 951782400) return t.year, t.month, t.day, t.yday, t.wday, t.isdst end)())
p("date Ex", os.date("!%Ex", 0))
p("date bad", pcall(os.date, "!%5"))
