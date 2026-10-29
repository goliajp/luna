-- rule: ldo.lua_resume.2
-- coroutine.resume at the C level limit: the error, the thread's status,
-- and resuming / closing it again, and the depth it fails at
local e = 0
local function h()
  e = e + 1
  local c = coroutine.create(h)
  local ok, m = coroutine.resume(c)
  if not ok and not R then
    R = { tostring(m), coroutine.status(c) }
    local ok2, m2 = coroutine.resume(c)
    R[#R + 1] = tostring(ok2) .. ":" .. tostring(m2)
    if coroutine.close then
      local ok3, m3 = coroutine.close(c)
      R[#R + 1] = tostring(ok3) .. ":" .. tostring(m3)
    end
  end
end
h()
print(e, table.concat(R, " | "))
R = nil
e = 0
pcall(h)
print(e, table.concat(R, " | "))
local d = 0
local function w() d = d + 1 return coroutine.wrap(w)() end
local ok, m = pcall(w)
print(ok, (tostring(m):gsub("^.*: ", "")), d)
