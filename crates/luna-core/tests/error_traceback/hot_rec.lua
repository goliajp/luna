local function fib(n) if n < 2 then if n < 0 then error("neg") end return n end return fib(n-1) + fib(n-2) end
for i = 1, 200 do fib(10) end
fib(-1)
