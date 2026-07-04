-- v2.12 CORPUS-III: numeric-for step=0 raises error.
local ok, err = pcall(function()
  for i = 1, 10, 0 do end
end)
local function strip(e) return e:match(": (.+)$") or tostring(e) end
print(ok, strip(err))
