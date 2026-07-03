-- v2.13 CORPUS-IV: ipairs starts at 1, stops at first nil,
-- ignores hash part and index 0.
local t = { [0] = "zero", "one", "two", nil, "four", extra = "hash" }
local seen = {}
for i, v in ipairs(t) do seen[#seen + 1] = i .. "=" .. v end
print(table.concat(seen, ","))
local empty = { [2] = "two" }
for i in ipairs(empty) do error("never") end
print("empty_ok")
