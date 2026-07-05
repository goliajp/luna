-- v2.15 P2.4 utf8: walking a string via offset.
local s = "aébcé"    -- 5 codepoints
local positions = {}
for i = 1, 5 do
  positions[i] = utf8.offset(s, i)
end
print(table.concat(positions, ","))
-- and past end
print(utf8.offset(s, 6))    -- points to byte after last char
