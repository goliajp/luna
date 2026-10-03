local function rec(n) if n == 0 then error("deep") end local r = rec(n - 1); return r end
rec(17)
