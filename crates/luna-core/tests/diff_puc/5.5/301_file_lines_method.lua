-- v2.14 CV.1: file:lines keeps the file open (unlike io.lines).
local f = io.tmpfile()
f:write("a\nbb\nccc\n")
f:seek("set", 0)
local n = 0
for line in f:lines() do n = n + 1 end
print(n)
print(io.type(f))
f:seek("set", 0)
for line in f:lines(2) do io.write("[", (line:gsub("\n", "N")), "]") end
print()
f:close()
