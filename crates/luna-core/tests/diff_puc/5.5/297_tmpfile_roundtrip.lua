-- v2.14 CV.1: io.tmpfile write/seek/read round-trip.
local f = io.tmpfile()
f:write("hello\nworld\n")
print(f:seek("set", 0))
print(f:read("l"))
print(f:read("a"))
print(f:seek("cur", 0))
f:close()
print(io.type(f))
