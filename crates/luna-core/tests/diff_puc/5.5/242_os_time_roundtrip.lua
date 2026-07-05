-- v2.13 CORPUS-IV: os.time(table) ↔ os.date("*t") round-trip
-- consistency (both sides run in the same process TZ, so the
-- comparison is deterministic within a run).
local t = os.time({ year = 2020, month = 6, day = 15, hour = 12, min = 30, sec = 45 })
print(math.type(t) ~= nil)
local d = os.date("*t", t)
print(d.year, d.month, d.day, d.hour, d.min, d.sec)
local t2 = os.time(d)
print(t2 == t)
print(os.date("!%Y-%m-%d", os.time({ year = 2000, month = 1, day = 1, hour = 12 })) ~= nil)
