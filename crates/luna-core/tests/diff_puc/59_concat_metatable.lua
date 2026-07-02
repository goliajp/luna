-- v2.11 CORPUS-II: table.concat + __tostring.
-- table.concat requires string/number elements, but list.concat is
-- often mixed via explicit tostring wrap.
local items = {1, "two", 3, "four"}
local strs = {}
for i, v in ipairs(items) do strs[i] = tostring(v) end
print(table.concat(strs, "-"))

-- non-primitive via explicit conversion
local V = {}
V.__tostring = function(v) return "<" .. v.tag .. ">" end
local a = setmetatable({tag="A"}, V)
local b = setmetatable({tag="B"}, V)
print(tostring(a), tostring(b))
print(tostring(a) .. "|" .. tostring(b))
