-- v2.15 P2.4: deeply nested do-end blocks.
local acc = 0
do
  local x = 1
  do
    local y = 2
    do
      local z = 3
      do
        local w = 4
        do
          local v = 5
          acc = x + y + z + w + v
        end
      end
    end
  end
end
print(acc)
