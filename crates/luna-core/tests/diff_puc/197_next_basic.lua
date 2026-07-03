-- v2.13 CORPUS-IV: next() on empty / single-entry tables (order-
-- independent assertions only).
print(next({}))
local t = { only = 1 }
local k, v = next(t)
print(k, v)
print(next(t, "only"))
local arr = { "x" }
print(next(arr))
print(next(arr, 1))
print((pcall(next, { a = 1 }, "missing_key")))
