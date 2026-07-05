-- v2.12 CORPUS-III: pcall wraps any error value.
-- Number error
local ok, err = pcall(function() error(42) end)
print(ok, err)   -- err is 42 (level=1 default doesn't add location for non-string)

-- Boolean error
local ok2, err2 = pcall(function() error(true) end)
print(ok2, err2)

-- Function error
local ok3, err3 = pcall(function() error(print) end)
print(ok3, type(err3))
