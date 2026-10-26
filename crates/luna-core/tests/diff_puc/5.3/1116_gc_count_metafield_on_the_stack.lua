-- luaL_getmetafield pushes the metatable, then the name it looks up, and
-- the push of the name checks for a collection: a finalizer the step runs
-- inside setmetatable finds both on setmetatable's stack, after its two
-- arguments
local seen = {}
local function look()
  for level = 2, 12 do
    local info = debug.getinfo(level, "f")
    if not info then break end
    if info.func == setmetatable then
      local vals, i = {}, 1
      while true do
        local n, v = debug.getlocal(level, i)
        if not n then break end
        vals[#vals + 1] = type(v) == "string" and v or type(v)
        i = i + 1
      end
      seen[table.concat(vals, " ")] = true
      return
    end
  end
end
local gcmt = {__gc = look}
local t = setmetatable({}, {})
for i = 1, 200000 do
  setmetatable({}, gcmt)
  setmetatable(t, {})
end
local out = {}
for k in pairs(seen) do out[#out + 1] = k end
table.sort(out)
print(#out > 0, table.concat(out, " | "))
