local function rec(n, i) if n == 0 then if i == 300 then error("deep " .. i) end return 0 end return rec(n - 1, i) + 1 end
for i = 1, 400 do rec(30, i) end
