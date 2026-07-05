-- v2.15 P2.4 utf8: ASCII is UTF-8 identity.
for i = 32, 126 do
  local c = utf8.char(i)
  local raw = string.char(i)
  if c ~= raw then print("differ at", i); break end
end
print("all 32-126 ASCII utf8 identity")
