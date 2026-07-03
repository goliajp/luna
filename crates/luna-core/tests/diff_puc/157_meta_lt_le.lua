-- v2.13 CORPUS-IV: __lt / __le metamethods drive < > <= >=.
local mt = {
  __lt = function(a, b) return (a.v or 0) < (b.v or 0) end,
  __le = function(a, b) return (a.v or 0) <= (b.v or 0) end,
}
local function n(v) return setmetatable({ v = v }, mt) end
print(n(1) < n(2), n(2) < n(1))
print(n(1) <= n(1), n(2) <= n(1))
print(n(1) > n(2), n(2) > n(1))
print(n(1) >= n(1), n(1) >= n(2))
