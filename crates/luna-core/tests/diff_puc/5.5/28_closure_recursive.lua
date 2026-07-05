-- v2.10 CORPUS: recursive local functions.
local function fact(n)
  if n <= 1 then return 1 end
  return n * fact(n - 1)
end
print(fact(5), fact(10))

-- mutual recursion via forward-decl
local iseven, isodd
iseven = function(n) if n == 0 then return true else return isodd(n - 1) end end
isodd = function(n) if n == 0 then return false else return iseven(n - 1) end end
print(iseven(10), isodd(7))
