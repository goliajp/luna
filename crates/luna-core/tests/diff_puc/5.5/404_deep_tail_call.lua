-- v2.15 P2.4: deep tail call chain (should not overflow via tco).
local function loop(n, acc)
  if n == 0 then return acc end
  return loop(n - 1, acc + 1)
end
print(loop(1000, 0))
print(loop(10000, 0))
