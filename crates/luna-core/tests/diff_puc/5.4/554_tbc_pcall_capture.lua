-- v2.15 P2.5: pcall catches error from body but still fires __close.
local closed = 0
local ok, err = pcall(function()
  local x <close> = setmetatable({}, {__close = function() closed = closed + 1 end})
  error("inner")
end)
local function strip(e) return e:match(": (.+)$") or tostring(e) end
print(ok, strip(err), closed)
