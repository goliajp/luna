local function cb(c) if c == "z" then error("cb") end return c end
local function g(s) return (string.gsub(s, ".", cb)) end
for i = 1, 1000 do g(i == 800 and "az" or "ab") end
