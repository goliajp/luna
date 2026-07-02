-- v2.11 CORPUS-II: upvalue vs local disambiguation.
local x = 100
local function outer()
  print(x)   -- 100 (upvalue capture)
  local x = 200
  print(x)   -- 200 (local shadow)
  do
    local x = 300
    print(x) -- 300
  end
  print(x)   -- 200
end
outer()
print(x)     -- 100
