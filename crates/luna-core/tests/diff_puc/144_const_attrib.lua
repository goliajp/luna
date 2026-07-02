-- v2.12 CORPUS-III: <const> attribute (5.4+). Assignment to a
-- const local is a compile error; only presence of "const" in
-- the message is compared (wording is dialect-sensitive).
local x <const> = 5
local y <const> = x + 1
print(x, y)
local f, err = load("local z <const> = 1; z = 2")
print(f == nil, err ~= nil and err:find("const") ~= nil)
