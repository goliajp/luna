-- v2.15 P2.5 (5.2): pcall behavior.
local function strip(e) return e:match(": (.+)$") or tostring(e) end
local ok, err = pcall(function() error("52err") end)
print(ok, strip(err))

local ok2, r = pcall(function() return "value" end)
print(ok2, r)
