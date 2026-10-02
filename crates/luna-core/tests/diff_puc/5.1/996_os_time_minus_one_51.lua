-- os.time on a time of -1: ≤5.2 return nil and leave the table alone,
-- 5.3+ write the normalised fields back before raising
local function show(t, ok, r)
  print(ok, r, t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday, t.isdst)
end
-- exactly -1 in UTC, reached two ways
local t = { year = 1970, month = 1, day = 1, hour = -1, min = 59, sec = 59 }
show(t, pcall(os.time, t))
t = { year = 1970, month = 1, day = 1, hour = 0, min = 0, sec = -1 }
show(t, pcall(os.time, t))
-- one second earlier succeeds
t = { year = 1970, month = 1, day = 1, hour = -1, min = 59, sec = 58 }
show(t, pcall(os.time, t))
-- the order of the reads and of the write-back, through a proxy
local log = {}
local src = { year = 1970, month = 1, day = 1, hour = 0, min = 0, sec = -1, isdst = false }
local p = setmetatable({}, {
  __index = function(_, k) log[#log + 1] = "get " .. k; return src[k] end,
  __newindex = function(_, k, v) log[#log + 1] = "set " .. k .. "=" .. tostring(v) end,
})
print(pcall(os.time, p))
print(table.concat(log, ", "))
