-- v2.14 CV.1: os.time table defaults — hour defaults to 12,
-- min/sec to 0; missing year/month/day error.
local t1 = os.time({ year = 2000, month = 6, day = 15 })
local t2 = os.time({ year = 2000, month = 6, day = 15, hour = 12, min = 0, sec = 0 })
print(t1 == t2)
print((pcall(os.time, { month = 1, day = 1 })))
print((pcall(os.time, { year = 2000, day = 1 })))
local d = os.date("*t", t1)
print(d.hour, d.min, d.sec)
