-- v2.12 CORPUS-III: stateless iterator function pattern.
local function range(state, i)
  i = i + 1
  if i <= state then return i, i * i end
end
for i, sq in range, 5, 0 do
  io.write(i, "^2=", sq, " ")
end
print()
