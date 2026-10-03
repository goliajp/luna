local depth = 0
local function f(n) if n == 0 then depth = depth + 1 if depth == 300 then error("deep hot") end return 0 end return f(n - 1) + 1 end
for i = 1, 400 do f(25) end
