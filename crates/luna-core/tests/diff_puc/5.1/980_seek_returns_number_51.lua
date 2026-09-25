-- 5.1 has one number type: file:seek's position is a double, so a large
-- offset prints in %.14g form, not as integer digits
local f = io.tmpfile()
f:write("abc")
print(f:seek("set", 2^60))
print(f:seek("cur"), f:seek("set"), f:seek("end"))
print(f:seek("set", 123456789012345))
f:close()
