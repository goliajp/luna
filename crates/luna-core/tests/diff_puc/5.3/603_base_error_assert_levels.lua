-- error / assert: which messages get a position, number messages on <=5.2,
-- assert's message handling per dialect, and error levels counted across
-- pcall, metamethods and tail calls.
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
show("error 42", error, 42)
show("error 42 lvl2", function() error(42, 2) end)
show("error 4.5", error, 4.5)
show("error table", function() local ok, e = pcall(error, {}) return type(e) end)
show("error nil", function() local ok, e = pcall(error) return e == nil end)
show("assert(false, 42)", assert, false, 42)
show("assert(false, {})", function() local ok, e = pcall(assert, false, {}) return type(e) end)
show("assert(false, nil)", function() local ok, e = pcall(assert, false, nil) return type(e) end)
show("assert()", assert)
show("assert in lua", function() assert(nil, 7) end)
show("assert in lua str", function() assert(false, "s") end)
show("assert returns", assert, 1, 2)
local function thrower(n) error("lv" .. n, n) end
for n = 1, 3 do
  show("pcall l" .. n, function() thrower(n) end)
  show("meta l" .. n, function() return setmetatable({}, {__index = function() thrower(n) end}).x end)
  show("tail l" .. n, function() return thrower(n) end)
  show("pcall pcall l" .. n, pcall, pcall, error, "pp", n)
end
