-- v2.13 CORPUS-IV: each loop iteration closes its OWN upvalue.
local fs = {}
for i = 1, 3 do
  local v = i * 10
  fs[i] = function() return v end
end
print(fs[1](), fs[2](), fs[3]())
-- while-loop block local: same per-iteration closure semantics
local gs, j = {}, 1
while j <= 3 do
  local w = j * 100
  gs[j] = function() return w end
  j = j + 1
end
print(gs[1](), gs[2](), gs[3]())
