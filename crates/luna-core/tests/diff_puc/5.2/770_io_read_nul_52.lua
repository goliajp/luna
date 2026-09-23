-- v3.1 iopkg: 5.2 still reads lines with fgets + strlen: a NUL ends the
-- chunk and swallows the newline after it ('*L' included); 5.3 switched
-- to a byte loop.
local base = os.tmpname()
local f = io.open(base, "wb")
f:write("a\0b\nc\0\nd\ne")
f:close()
f = io.open(base, "rb")
local l = f:read("*L")
print(#l, (l:gsub("%z", "\\0"):gsub("\n", "\\n")))
print(f:read("*L"), f:read("*l"), f:read("*a"))
f:close()
os.remove(base)
