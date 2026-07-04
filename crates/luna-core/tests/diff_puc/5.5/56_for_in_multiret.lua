-- v2.11 CORPUS-II: for-in iterator with multi-return.
local function iter(state, i)
  if i < state then return i + 1, i * 10 end
end
for i, v in iter, 3, 0 do
  io.write(i, "=", v, " ")
end
print()

-- ipairs standard
for i, v in ipairs({"a","b","c"}) do io.write(i, "=", v, " ") end
print()
