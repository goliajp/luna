-- tostring: __tostring results per dialect (<=5.1 anything, 5.2 renders a
-- number, 5.3+ accepts strings and numbers), __name from 5.3, and C
-- functions render as "function: <address>".
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
local function ts(label, v)
  local ok, s = pcall(tostring, v)
  local shown = type(s) == "string" and clean((s:gsub("0x%x+", "ADDR"))) or type(s)
  print(label, ok, type(s), shown)
end
ts("mm num", setmetatable({}, {__tostring = function() return 42 end}))
ts("mm nil", setmetatable({}, {__tostring = function() end}))
ts("mm tbl", setmetatable({}, {__tostring = function() return {} end}))
ts("name", setmetatable({}, {__name = "MyType"}))
ts("name num", setmetatable({}, {__name = 42}))
ts("c function", print)
ts("int", 7)
