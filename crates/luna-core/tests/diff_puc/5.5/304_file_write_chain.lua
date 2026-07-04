-- v2.14 CV.1: file:write returns the file — chained writes.
local f = io.tmpfile()
local r = f:write("a"):write("b"):write(1, 2)
print(r == f)
f:seek("set", 0)
print(f:read("a"))
f:close()
