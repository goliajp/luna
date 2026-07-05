-- v2.14 CV.1: file:setvbuf returns TRUE (luaL_fileresult), not
-- the file; flush behaves the same.
local f = io.tmpfile()
print(f:setvbuf("no") == true)
print(f:setvbuf("full") == true)
print(f:setvbuf("line") == true)
print(f:setvbuf("full", 4096) == true)
print(f:flush() == true)
f:close()
