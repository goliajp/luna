-- v2.15 P2.5 (5.1): local scoping.
local x = 10
do
  local x = 20
  print(x)
end
print(x)
