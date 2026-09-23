-- v3.1 debug slice: luaO_chunkid's short_src for long chunk names, per
-- version (5.1 keeps 52 bytes of a path and 43 of a string, 5.2+ 56 and
-- 45; 5.1 also stops a string at '\r'), at and around each boundary.
local loadstr = loadstring or load
local function src(name)
  local f = assert(loadstr("return debug.getinfo(1, 'S').short_src", name))
  return f()
end
local function rep(n) return string.rep("abcdefghij", 10):sub(1, n) end
for _, n in ipairs{50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 70} do
  print("@" .. n, src("@" .. rep(n)))
  print("=" .. n, src("=" .. rep(n)))
end
for _, n in ipairs{40, 42, 43, 44, 45, 46, 60} do
  print("str" .. n, src(rep(n)))
end
print("nl", src("first line\nsecond"))
print("cr", src("first line\rsecond"))
print("long nl", src(rep(50) .. "\nx"))
local f, msg = loadstr("x = = 1", "@" .. rep(70))
print("syntax", msg)
print("syntax =", select(2, loadstr("x = = 1", "=" .. rep(90))))
print("syntax str", select(2, loadstr("x = = 1 -- " .. rep(90))))
for _, n in ipairs{70, 71, 72, 73, 74, 80} do
  print("syntax @" .. n, select(2, loadstr("x = = 1", "@" .. rep(n))))
end
