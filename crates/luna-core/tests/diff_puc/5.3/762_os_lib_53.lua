-- v3.1 iopkg: the 5.3 os library. os.time wants integer fields (reading
-- from the seconds upwards), bounds them at INT_MAX/2, writes the
-- normalised fields back and returns an integer; an unrepresentable date
-- is a "time result" error; difftime requires both times as integers;
-- strftime conversions follow C99.
local function clean(s) return (tostring(s):gsub("[^%s]*:%d+: ", "POS: ")) end
local function p(name, ...) local t = {...}; for i = 1, select("#", ...) do t[i] = clean(t[i]) end print(name, select("#", ...), table.concat(t, " ")) end
local t0 = 86400 * 45 + 3600 * 15 + 61
p("date C99", os.date("!%a %A %b %B %c|%C %d %D %e %F %g %G %h %H %I %j %m %M %n %p %r %R %S %t %T %u %U %V %w %W %x %X %y %Y %%", t0))
for _, t in ipairs{0, -1, 951782400, 1234567890, 253402300799, 1609459200, 1293753600} do
  p("date " .. t, os.date("!%Y-%m-%d %j %U %W %V %G %g %u %w", t))
end
p("date unrepresentable", pcall(os.date, "!%Y", 2^62))
p("date float time", pcall(os.date, "!%Y", 1.5))
local ref = os.time{year = 2000, month = 1, day = 1, hour = 0}
p("time type", math.type(ref))
p("time not integer", pcall(os.time, {year = 2000, month = 1, day = 1.5}))
p("time string", pcall(os.time, {year = 2000, month = 1, day = "x"}))
p("time bound", pcall(os.time, {year = 2000, month = 1, day = 1073741824}))
p("time missing", pcall(os.time, {}))
local nt = {year = 2000, month = 14, day = 35, hour = 25, min = 61, sec = 61}
local d = os.time(nt) - ref
p("time normalises", d, nt.year, nt.month, nt.day, nt.hour, nt.min, nt.sec, nt.yday, nt.wday, nt.isdst)
p("difftime one", pcall(os.difftime, 10))
p("difftime float", pcall(os.difftime, 1.5, 1))
p("difftime", os.difftime(10, 3))
p("setlocale", pcall(os.setlocale, nil, 3))
