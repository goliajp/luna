-- os.time with isdst: PUC hands tm_isdst (nil -> -1, otherwise the
-- truth value) to mktime; in UTC, which has no daylight saving time,
-- a true isdst moves the result one hour back
local function show(t, ok, r)
  local dst = t.isdst
  if type(dst) == "table" then dst = "a table" end
  print(ok, r, t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday, dst)
end
for _, dst in ipairs({ "nil", false, true, 0, "", {} }) do
  local t = { year = 1970, month = 1, day = 1, hour = 0, min = 0, sec = 0 }
  if dst ~= "nil" then t.isdst = dst end
  io.write(type(dst), " ")
  show(t, pcall(os.time, t))
end
-- one second before the epoch, an hour further back
local t = { year = 1970, month = 1, day = 1, hour = 0, min = 0, sec = -1, isdst = true }
show(t, pcall(os.time, t))
-- a time of -1 reached through the shift
t = { year = 1970, month = 1, day = 1, hour = 1, min = 0, sec = -1, isdst = true }
show(t, pcall(os.time, t))
t = { year = 1970, month = 1, day = 1, hour = 1, min = 0, sec = 0, isdst = true }
show(t, pcall(os.time, t))
-- a summer date and a leap day
t = { year = 2024, month = 7, day = 15, hour = 12, min = 30, sec = 15, isdst = true }
show(t, pcall(os.time, t))
t = { year = 2024, month = 3, day = 1, hour = 0, min = 30, isdst = true }
show(t, pcall(os.time, t))
t = { year = 2024, month = 3, day = 1, hour = 0, min = 30, isdst = false }
show(t, pcall(os.time, t))
-- isdst read through __index
local p = setmetatable({}, { __index = function(_, k)
  if k == "isdst" then return 1 end
  return ({ year = 2000, month = 1, day = 1, hour = 0 })[k]
end })
print(pcall(os.time, p))
