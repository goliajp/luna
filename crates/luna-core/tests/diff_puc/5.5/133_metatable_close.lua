-- v2.12 CORPUS-III: __close raises inside close is re-raised.
local ok, err = pcall(function()
  local x <close> = setmetatable({}, {__close = function() error("close-err") end})
end)
local function strip(e) return e:match(": (.+)$") or tostring(e) end
print(ok, strip(err))
