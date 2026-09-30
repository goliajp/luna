-- table writes through every write opcode: overwriting a present key never
-- calls __newindex, a nil slot does, and the chain, deletion and key
-- normalization all behave as before
local log = {}
local function show(...)
  local t = {}
  for i = 1, select("#", ...) do t[#t + 1] = tostring((select(i, ...))) end
  print(table.concat(t, " "))
end
local store = {}
local mt = {__newindex = function(t, k, v) log[#log + 1] = tostring(k) .. "=" .. tostring(v); rawset(t, k, v) end}
local o = setmetatable({a = 1, [1] = "x", [3] = "z"}, mt)
o.a = 2          -- present: no __newindex
o.b = 3          -- absent: __newindex
o.b = 4          -- now present
o[1] = "y"       -- array part, present
o[2] = "w"       -- absent
o[2.0] = "w2"    -- float key normalizes to 2, present
o.a = nil        -- delete a present key: no __newindex
o.a = 5          -- slot now nil: __newindex again
local k = "ke" .. "y"
o[k] = 1
o.key = 2
show(o.a, o.b, o[1], o[2], o.key, table.concat(log, ","))
-- __newindex as a table: the write lands there, not in the proxy
local proxy = setmetatable({}, {__newindex = store})
proxy.x = 1
proxy[1] = 2
show(rawget(proxy, "x"), store.x, store[1])
-- chains of tables
local last = {}
local c = setmetatable({}, {__newindex = setmetatable({}, {__newindex = last})})
c.deep = "d"
show(last.deep)
-- bad keys still raise
local function err(f)
  local ok, e = pcall(f)
  return ok, (string.gsub(tostring(e), "^.-:%d+: ", ""))
end
show(err(function() local t = {} t[nil] = 1 end))
show(err(function() local t = {} t[0/0] = 1 end))
-- globals through _ENV's metatable
local seen = {}
setmetatable(_G, {__newindex = function(t, k2, v) seen[#seen + 1] = k2; rawset(t, k2, v) end})
newglobal = 1
newglobal = 2
show(newglobal, table.concat(seen, ","))
setmetatable(_G, nil)
newglobal = nil
-- counts and borders after mixed writes
local arr = {}
for i = 1, 20 do arr[i] = i end
for i = 1, 20, 3 do arr[i] = i * 10 end
arr[21] = 21
arr[5] = nil
show(arr[1], arr[4], arr[21], arr[5], #arr == 21 or #arr == 4)
