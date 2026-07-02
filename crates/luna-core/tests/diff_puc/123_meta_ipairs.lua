-- v2.12 CORPUS-III: 5.3+ ipairs respects __index — raw part
-- 1..3 first, then __index supplies 4..5, stops at nil (k=6).
local t = setmetatable({ 1, 2, 3 }, {
  __index = function(_, k)
    if k <= 5 then return "meta_" .. k end
  end,
})
local parts = {}
for i, v in ipairs(t) do parts[#parts + 1] = tostring(v) end
print(#parts, table.concat(parts, ","))
