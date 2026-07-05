-- v2.15 P2.4 utf8: operations on empty strings.
print(utf8.len(""))                    -- 0
print(#utf8.char())                    -- 0 (no args = empty)
local n = 0
for _ in utf8.codes("") do n = n + 1 end
print(n)                                -- 0
