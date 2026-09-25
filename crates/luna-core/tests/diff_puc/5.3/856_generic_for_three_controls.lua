-- Before 5.4 a generic for keeps three control values (PUC forlist's
-- adjust_assign to 3): a fourth is evaluated and dropped, never closed,
-- and a `return f()` in the body is still a tail call.
local n, seen = 0, 0
local function fourth() seen = seen + 1 return 42 end
for k in next, {1, 2}, nil, fourth() do n = n + 1 end
print(n, seen)
local function g() return debug.getinfo(2, "S").what end
local function loop() for k in pairs({1}) do return g() end end
print(loop())
