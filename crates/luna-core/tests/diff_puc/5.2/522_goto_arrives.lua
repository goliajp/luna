-- v2.14 HD 5.2 seed: goto/labels arrive.
local sum = 0
for i = 1, 5 do
  if i == 3 then goto continue end
  sum = sum + i
  ::continue::
end
print(sum)
