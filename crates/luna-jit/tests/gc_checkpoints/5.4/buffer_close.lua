-- a library function's result while its buffer's box is closed: the
-- string is on the stack then, under the call `lua_closeslot` makes
for n = 6144, 6145 do
  local x = "a" .. string.rep("x", n) .. "b"
  print(#x, x:sub(1, 3), x:sub(-2))
end
for _, n in ipairs({3000, 9000}) do
  local s = string.format("%s%s", string.rep("y", n), "z")
  print(#s, s:sub(-2))
end
