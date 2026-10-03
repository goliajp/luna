local function rec(n) if n == 0 then local x = nil; return x.y end return (pcall(rec, n - 1)) and error("x") or select(2, pcall(rec, n-1)) end
local function r2(n) if n == 0 then error({}) end table.sort({2,1,3}, function(a, b) r2(n - 1) return a < b end) end
r2(15)
