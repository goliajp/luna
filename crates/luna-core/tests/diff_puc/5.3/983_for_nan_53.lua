-- A numeric for loop with a NaN init, limit or step. 5.4 and 5.5 skip a
-- float loop only when `0 < step ? limit < init : init < limit`, which a NaN
-- makes false, so the body runs once; 5.3 tests `idx <= limit` first and
-- runs nothing. A NaN limit of an integer loop is always a skip.
local function runs(f) local n = 0 f(function() n = n + 1 end) return n end
print("nan init", runs(function(b) for i = 0/0, 1 do b() end end))
print("nan init, neg step", runs(function(b) for i = 0/0, 1, -1 do b() end end))
print("nan limit", runs(function(b) for i = 1, 0/0 do b() end end))
print("nan limit, float", runs(function(b) for i = 1.0, 0/0 do b() end end))
print("nan step", runs(function(b) for i = 1, 2, 0/0 do b() end end))
print("nan step, init above", runs(function(b) for i = 2, 1, 0/0 do b() end end))
