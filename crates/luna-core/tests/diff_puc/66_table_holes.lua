-- v2.11 CORPUS-II: ipairs stops at nil hole.
local t = {10, 20, 30}
t[5] = 50
-- ipairs stops at nil hole (t[4] = nil)
local n = 0
for _ in ipairs(t) do n = n + 1 end
print(n)   -- 3

-- explicit index access still works
print(t[3], t[4], t[5])
