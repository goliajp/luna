-- string.format asks for a `%f` item's extra room only after the
-- argument is there and the specification is read: an error before that
-- leaves no bigger buffer behind
collectgarbage("stop")
local function bytes(f, ...)
  local a = collectgarbage("count")
  local ok, e = pcall(f, ...)
  if ok then e = #e end
  return ok, e, (collectgarbage("count") - a) * 1024
end
local head = string.rep("x", 700)
print(bytes(string.format, head .. "%f"))
print(bytes(string.format, head .. "%0000000000000000000000f", 1))
print(bytes(string.format, head .. "%5.2f|", 1.5))
print(bytes(string.format, head .. "%f%d", 1.5))
