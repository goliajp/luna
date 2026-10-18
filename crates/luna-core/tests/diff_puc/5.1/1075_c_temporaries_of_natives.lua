-- the values a C function has on its stack below a function it calls back
local function show(tag, lvl)
  local out = {}
  for i = 1, 12 do
    local name, v = debug.getlocal(lvl or 3, i)
    if not name then break end
    local tv = type(v)
    out[#out + 1] = (tv == "string" or tv == "number" or tv == "boolean") and tostring(v) or tv
  end
  print(tag, #out, table.concat(out, " "))
end
local mt = {__tostring = function(...) show("tostring") return "s" end}
local o = setmetatable({}, mt)
tostring(o)
print(o)
pcall(string.format, "%s", o)
local n = 0
table.sort({3, 1, 2}, function(a, b) n = n + 1 if n == 1 then show("sort") end return a < b end)
string.gsub("abc", "(b)", function(...) show("gsub") end)
string.gsub("abc", "%w", setmetatable({}, {__index = function(...) show("gsubidx") end}), 1)
local p = setmetatable({}, {__index = function(t, i) if i == 1 then show("concat") end if i <= 2 then return "x" end end})
pcall(table.concat, p, ",", 1, 2)
if _VERSION >= "Lua 5.2" then
  pcall(table.unpack or unpack, setmetatable({}, {__index = function(t, i) if i == 1 then show("unpack") end end}), 1, 2)
  for _ in pairs(setmetatable({}, {__pairs = function(...) show("pairs") return function() end end})) do end
end
if _VERSION >= "Lua 5.3" then
  for _ in ipairs(setmetatable({}, {__index = function(t, i) if i == 1 then show("ipairs") end end})) do end
end
local main = coroutine.running()
if main then
  local f = coroutine.wrap(function(...) show("wrap", 1) end)
  local function sh() 
    local out = {}
    for i = 1, 12 do local name, v = debug.getlocal(main, 0, i) if not name then break end out[#out+1] = type(v) end
    print("wrapmain", #out, table.concat(out, " "))
  end
  coroutine.wrap(function(...) sh() end)("a", "b")
end
xpcall(function() error("e", 0) end, function(...) show("handler") end)
