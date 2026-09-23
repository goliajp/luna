-- v3.1 bytecode: a store through a nil upvalue names the upvalue.
local uv
local function f() uv.field = 1 end
f()
