-- v2.11 CORPUS-II: error() with structured (non-string) argument.
local function throw(code)
  error({code = code, message = "err" .. code})
end
local ok, err = pcall(throw, 404)
print(ok, err.code, err.message)

-- xpcall preserves the table
local ok2, err2 = xpcall(function() throw(500) end, function(e) return {passthrough = e} end)
print(ok2, err2.passthrough.code, err2.passthrough.message)
