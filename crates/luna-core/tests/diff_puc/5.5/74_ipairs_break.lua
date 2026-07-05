-- v2.11 CORPUS-II: ipairs + break.
local t = {10, 20, 30, 40, 50}
local sum = 0
for i, v in ipairs(t) do
  if v > 30 then break end
  sum = sum + v
end
print(sum)  -- 60 (10+20+30)

-- continue via goto
sum = 0
for i, v in ipairs(t) do
  if i == 2 then goto continue end
  sum = sum + v
  ::continue::
end
print(sum)  -- 130 (10+30+40+50)
