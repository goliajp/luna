-- v2.14 CV.2: indexing a nil upvalue — varinfo says upvalue.
local uv
local function f() return uv.field end
f()
