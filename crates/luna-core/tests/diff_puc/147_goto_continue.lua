-- v2.12 CORPUS-III: goto/labels — continue pattern + backward
-- jump loop.
local sum = 0
for i = 1, 5 do
  if i % 2 == 0 then goto continue end
  sum = sum + i
  ::continue::
end
print(sum)
local n = 0
::top::
n = n + 1
if n < 3 then goto top end
print(n)
