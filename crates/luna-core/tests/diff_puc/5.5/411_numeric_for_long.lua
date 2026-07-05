-- v2.15 P2.4: long numeric-for loop.
local sum = 0
for i = 1, 10000 do sum = sum + i end
print(sum)

sum = 0
for i = 1, 1000 do
  for j = 1, 10 do sum = sum + 1 end
end
print(sum)
