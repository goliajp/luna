-- v2.15 P2.4: deep call stack near soft limit.
local function rec(n) if n == 0 then return 0 end; return 1 + rec(n - 1) end
print(rec(50))
print(rec(150))
