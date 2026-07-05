-- v2.15 P2.4: forward-declared mutual recursion chain.
local a, b, c
a = function(n) if n == 0 then return 0 end; return b(n - 1) + 1 end
b = function(n) if n == 0 then return 0 end; return c(n - 1) + 2 end
c = function(n) if n == 0 then return 0 end; return a(n - 1) + 3 end
print(a(10), b(10), c(10))
