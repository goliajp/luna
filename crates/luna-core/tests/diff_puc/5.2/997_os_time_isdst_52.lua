-- os.time with isdst: PUC hands tm_isdst (nil -> -1, otherwise the
-- truth value) to mktime. Only the cases mktime treats the same on every
-- C library are here; a true isdst in UTC depends on the C library
-- (newer glibc moves the result an hour back, older glibc fails), and
-- luna's own choice for it is pinned in tests/it/os_time_isdst.rs
local function show(t, ok, r)
  print(ok, r, t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday, t.isdst)
end
for _, dst in ipairs({ "nil", false }) do
  local t = { year = 1970, month = 1, day = 1, hour = 0, min = 0, sec = 0 }
  if dst ~= "nil" then t.isdst = dst end
  io.write(type(dst), " ")
  show(t, pcall(os.time, t))
end
local t = { year = 2024, month = 3, day = 1, hour = 0, min = 30, isdst = false }
show(t, pcall(os.time, t))
