-- v3.1 iopkg: 5.1 reads a line with fgets and measures it with strlen, so
-- a NUL cuts the line there and the newline after it is lost: the line
-- runs on into the next one.
local base = os.tmpname()
local f = io.open(base, "wb")
f:write("a\0b\nc\0\nd\ne")
f:close()
f = io.open(base, "rb")
local l = f:read("*l")
print(#l, (l:gsub("%z", "\\0")))
print(f:read("*l"), f:read("*l"), f:read("*l"))
f:close()
local t = {}
for line in io.lines(base) do t[#t + 1] = (line:gsub("%z", "\\0")) end
print(table.concat(t, "|"))
os.remove(base)
