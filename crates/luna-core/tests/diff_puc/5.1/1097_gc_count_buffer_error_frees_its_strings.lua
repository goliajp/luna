-- a 5.1 luaL_Buffer keeps the pieces it has filled on the stack: an error
-- in the middle of a library function that builds one lets them go. With
-- a pause of 0 and a large step multiplier each check runs a whole cycle,
-- and each finalizer in the chain drops a proxy whose finalizer is the
-- next, so the k-th check after the call starts raises
local function big(c) return string.rep(c, 20000) end
local path = os.tmpname()
local fh = assert(io.open(path, "w"))
fh:write(big("r"))
fh:close()
local function chain(k)
  local p = newproxy(true)
  if k == 1 then
    getmetatable(p).__gc = function() error("gc") end
  else
    getmetatable(p).__gc = function() chain(k - 1) end
  end
end
local function count()
  collectgarbage()
  collectgarbage()
  return collectgarbage("count") * 1024
end
-- each: what is made before the chain is armed, and the call
local calls = {
  {"format", function() return big("f") end, function(s)
    return string.format("%s%s", s, s)
  end},
  {"gsub", function() return big("g") end, function(s)
    return (string.gsub(s, "g", "hh"))
  end},
  {"concat", function() return {big("c"), big("c"), big("c")} end, table.concat},
  {"date", function() return big("d") end, os.date},
  {"read", function() return io.open(path) end, function(f)
    local s = f:read("*a")
    f:close()
    return s
  end},
  {"lines", function()
    local f = io.open(path)
    return {f, f:lines()}
  end, function(t)
    local s = t[2]()
    t[1]:close()
    return s
  end},
  {"upper", function() return big("u") end, string.upper},
}
for _, c in ipairs(calls) do
  local base = count()
  for k = 1, 8 do
    local arg = c[2]()
    collectgarbage("setpause", 0)
    collectgarbage("setstepmul", 1000000)
    collectgarbage()
    chain(k)
    local ok, r = pcall(c[3], arg)
    r = ok and #r or r
    arg = nil
    collectgarbage("setpause", 200)
    collectgarbage("setstepmul", 200)
    for _ = 1, 8 do pcall(collectgarbage) end
    print(c[1], k, ok, r, count() - base)
  end
end
os.remove(path)
