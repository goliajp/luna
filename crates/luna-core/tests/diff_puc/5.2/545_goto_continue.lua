-- v2.15 P2.5 (5.2): continue via goto.
local sum = 0
for i = 1, 10 do
  if i % 2 == 0 then goto cont end
  sum = sum + i
  ::cont::
end
print(sum)   -- 25
