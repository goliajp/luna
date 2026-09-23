-- An __index/__newindex chain that loops: 5.1/5.2 give up after 100
-- links with "loop in gettable/settable", 5.3+ after 2000.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
e("local a = {} setmetatable(a, {__index = a}) return a.x")
e("local a = {} setmetatable(a, {__newindex = a}) a.x = 1")
e([[local t = {} local cur = t
for i = 1, 150 do local n = {} setmetatable(cur, {__index = n}) cur = n end
cur.x = 'found' return t.x]])
