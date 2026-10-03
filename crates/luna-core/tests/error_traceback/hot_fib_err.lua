local cnt = 0
local function fib(n) cnt = cnt + 1 if cnt == 50000 then error("fib " .. n) end if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
for i = 1, 400 do fib(10) end
