-- v2.13 CORPUS-IV: proper tail calls — deep recursion without
-- stack growth; non-tail form is bounded by the stack.
local function loop(n)
  if n == 0 then return "bottom" end
  return loop(n - 1)
end
print(loop(1000000))
local function mutual_a(n) if n == 0 then return "a0" end return mutual_b(n - 1) end
function mutual_b(n) if n == 0 then return "b0" end return mutual_a(n - 1) end
print(mutual_a(99999))
print(mutual_a(100000))
