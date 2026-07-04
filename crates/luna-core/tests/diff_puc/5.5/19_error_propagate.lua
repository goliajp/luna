-- v2.10 CORPUS: error propagation through nested pcall.
local function strip(e) return e:match(": (.+)$") or tostring(e) end
local function deep(n)
  if n == 0 then error("bottom") end
  return deep(n - 1)
end
local ok, err = pcall(deep, 5)
print(ok, strip(err))

-- error with non-string value
local ok2, err2 = pcall(function() error({code = 42, msg = "structured"}) end)
print(ok2, err2.code, err2.msg)

-- error level=2 (blames caller)
local function bad() error("level2-msg", 2) end
local ok3, err3 = pcall(bad)
print(ok3, strip(err3))
