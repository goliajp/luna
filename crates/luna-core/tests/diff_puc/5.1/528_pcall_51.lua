-- v2.15 P2.5 (5.1): pcall arity.
local function strip(e) return type(e) == "string" and (e:match(": (.+)$") or e) or tostring(e) end
local ok, err = pcall(function() error("51err") end)
print(ok, strip(err))
