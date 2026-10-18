-- a finalizer that allocates: the threshold it runs under (twice the
-- count when it starts) lets steps run inside it once it has allocated
-- that much, and a whole cycle clears a weak table's entry
local weak = setmetatable({}, {__mode = "v"})
local steps
local p = newproxy(true)
debug.getmetatable(p).__gc = function()
  weak[1] = {}
  local keep = {}
  for i = 1, 200000 do
    keep[#keep + 1] = {}
    if weak[1] == nil then
      steps = i
      return
    end
  end
  steps = -1
end
p = nil
for i = 1, 100000 do
  local _ = {i}
  if steps then break end
end
print(steps)
