-- v2.15 P2.5: <const> local rejects assignment.
local x <const> = 42
print(x)
local ok, err = pcall(load("local y <const> = 10; y = 20"))
print(ok)
