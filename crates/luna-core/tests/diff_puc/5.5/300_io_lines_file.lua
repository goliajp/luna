-- v2.14 CV.1: io.lines over a named temp file + cleanup.
local name = os.tmpname()
local f = assert(io.open(name, "w"))
f:write("alpha\nbeta\ngamma\n")
f:close()
local got = {}
for line in io.lines(name) do got[#got + 1] = line end
print(#got, table.concat(got, "|"))
for line in io.lines(name, "L") do io.write("[", (line:gsub("\n", "N")), "]") end
print()
print(os.remove(name))
print(io.open(name, "r") == nil)
