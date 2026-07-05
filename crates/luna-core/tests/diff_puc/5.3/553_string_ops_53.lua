-- v2.15 P2.5 (5.3): string library w/ integer semantics.
print(string.format("%d", 42))
print(string.format("%d", 42.0))   -- ok, converts
local ok = pcall(string.format, "%d", 42.5)
print(ok)                            -- false (non-int float rejected)
