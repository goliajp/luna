-- v2.14 CV.1: reads at/past EOF — "a" gives "", others nil.
local f = io.tmpfile()
f:write("xy")
f:seek("set", 0)
print(f:read("a"))
print(f:read("a") == "")
print(f:read("l"), f:read("L"), f:read(1), f:read("n"))
f:close()
