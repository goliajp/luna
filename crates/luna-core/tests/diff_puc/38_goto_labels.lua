-- v2.10 CORPUS: goto + labels (5.2+).
local n = 0
::start::
n = n + 1
if n < 3 then goto start end
print(n)  -- 3

-- forward goto through break analog
for i = 1, 5 do
  for j = 1, 5 do
    if i * j == 6 then goto done end
  end
end
::done::
print("done")

-- continue idiom
local sum = 0
for i = 1, 10 do
  if i % 2 == 0 then goto continue end
  sum = sum + i
  ::continue::
end
print(sum)  -- 25 (odd sum 1..10)
