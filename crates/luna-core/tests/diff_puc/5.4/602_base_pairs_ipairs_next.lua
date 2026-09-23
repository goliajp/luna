-- pairs / ipairs / next per dialect: __pairs (5.2+) result counts, __ipairs
-- (5.2 and 5.3's default build), 5.3+ accepting any value, <=5.2 ipairs
-- reading raw, and the end-of-sequence result count.
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
local pm = setmetatable({}, {__pairs = function() return 1, 2, 3, 4, 5 end})
print("pairs mm", select("#", pairs(pm)), render(pairs(pm)))
print("pairs plain", select("#", pairs({})))
print("pairs is next", (pairs({})) == next)
show("pairs(1)", function() return type((pairs(1))) end)
show("pairs(nil)", pairs, nil)
local im = setmetatable({}, {__ipairs = function() return 1, 2, 3, 4 end})
print("ipairs mm", render(ipairs(im)))
show("ipairs('abc')", function() return type((ipairs("abc"))) end)
show("ipairs(nil)", ipairs, nil)
local ix = setmetatable({}, {__index = function(_, i) if i <= 3 then return i * 10 end end})
local seen = {}
for i, v in ipairs(ix) do seen[#seen + 1] = i .. "=" .. v end
print("ipairs __index", table.concat(seen, " "))
local it = ipairs({})
print("iter end", select("#", it({10}, 1)))
show("iter bad ctl", it, {10}, "x")
show("iter no ctl", it, {10})
show("iter float ctl", it, {10, 20}, 1.5)
show("iter string ctl", it, {10, 20}, "1")
show("iter non-table", it, "abc", 0)
show("next float key", next, {5, 6}, 1.0)
show("next bad key", next, {}, "k")
