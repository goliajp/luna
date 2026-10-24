-- the output follows where the automatic collection steps run
-- 5.1's string.rep builds an empty result in a buffer, which checks for no
-- collection: the steps fall at the tables' checkpoints alone
local out = {}
for i = 1, 200 do
  local t = {i}
  local s = string.rep("x", 0)
  out[#out + 1] = collectgarbage("count") * 1024
end
local sum = 0
for i = 1, #out do sum = sum + out[i] * i end
print(string.format("%.0f", sum))
