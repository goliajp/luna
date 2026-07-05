-- v2.14 CV.2: non-tail infinite recursion — "stack overflow"
-- with a position prefix.
local function f() return f() + 1 end
f()
