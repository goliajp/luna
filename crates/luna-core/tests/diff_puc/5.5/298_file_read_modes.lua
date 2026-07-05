-- v2.14 CV.1: file:read mode matrix — "l" strips EOL, "L" keeps
-- it, "a" slurps, numeric reads count bytes, "n" parses numbers.
local f = io.tmpfile()
f:write("line1\nline2\n42 3.5\nrest")
f:seek("set", 0)
print(f:read("l"))
print((f:read("L")):gsub("\n", "<NL>"))
print(f:read("n"), f:read("n"))
f:read("l")
print(f:read(2))
print(f:read("a"))
print(f:read("a") == "", f:read(1))
f:close()
