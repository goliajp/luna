-- 5.1 getfenv / setfenv / newproxy argument handling.
-- Error messages are compared without their position ("@" marks one)
-- and without 5.2's hash-order-dependent "_G." qualifier.
local function clean(s)
  s = tostring(s):gsub("'_G%.", "'")
  local pos = s:match("^[^:\n]+:%d+: ") and "@" or ""
  return pos .. s:gsub("^[^:\n]+:%d+: ", "")
end
local function render(...)
  local out = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    local t = type(v)
    if t == "string" then out[#out + 1] = clean(v)
    elseif t == "number" or t == "boolean" or t == "nil" then out[#out + 1] = tostring(v)
    else out[#out + 1] = "<" .. t .. ">" end
  end
  return table.concat(out, " | ")
end
local function show(label, ...) print(label, render(pcall(...))) end
show("getfenv('1')", function() return getfenv("1") == _G end)
show("getfenv(1.5)", function() return getfenv(1.5) == _G end)
show("getfenv(-1)", getfenv, -1)
show("getfenv(99)", getfenv, 99)
show("getfenv('x')", getfenv, "x")
show("setfenv(1) in fn", function() setfenv(1, {}) return 1 end)
show("setfenv level C", function() return pcall(setfenv, 1, {}) end)
show("setfenv(99)", setfenv, 99, {})
show("setfenv(f)", setfenv, function() end)
show("setfenv(0)", function() return select("#", setfenv(0, getfenv(0))) end)
show("setfenv C function", setfenv, string.len, {})
show("newproxy()", function() return type(newproxy()) end)
show("newproxy(true)", function() return type(getmetatable(newproxy(true))) end)
show("newproxy(proxy)", function() local p = newproxy(true) return getmetatable(newproxy(p)) == getmetatable(p) end)
show("newproxy(nonproxy)", newproxy, newproxy(false))
show("newproxy(io)", newproxy, io.stdout)
show("newproxy(1)", newproxy, 1)
show("pairs is not next", function() return (pairs({})) == next end)
