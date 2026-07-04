-- v2.14 CV.1: io.type states.
local f = io.tmpfile()
print(io.type(f))
f:close()
print(io.type(f))
print(io.type("not a file"), io.type(42), io.type({}))
print(io.type(io.stdout))
