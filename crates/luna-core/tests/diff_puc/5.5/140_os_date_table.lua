-- v2.12 CORPUS-III: os.date("!*t") table form, fixed epoch.
local t = os.date("!*t", 86400)
print(t.year, t.month, t.day)
print(t.hour, t.min, t.sec)
print(t.wday, t.yday, t.isdst)
local u = os.date("!*t", 1000000000)
print(u.year, u.month, u.day, u.hour)
