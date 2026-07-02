-- v2.10 CORPUS: error + pcall + xpcall.
-- Strip source-location prefix (harness stdin vs Vm::eval chunk-name
-- differs) using string.match on ": " suffix.
local function strip(e) return e:match(": (.+)$") or tostring(e) end
local ok, err = pcall(function() error("boom") end)
print(ok, strip(err))

local ok2 = pcall(function() return 42 end)
print(ok2)

local ok3, msg = xpcall(function() error("x") end, function(e) return "handled:" .. strip(e) end)
print(ok3, msg)

local ok4, a, b, c = pcall(function() return 1, 2, 3 end)
print(ok4, a, b, c)

local ok5, err5 = pcall(function() assert(false, "assert-msg") end)
print(ok5, strip(err5))
