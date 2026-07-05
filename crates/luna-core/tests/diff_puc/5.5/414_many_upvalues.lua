-- v2.15 P2.4: closure with many upvalues.
local function makeadd(...)
  local args = table.pack(...)
  return function()
    local sum = 0
    for i = 1, args.n do sum = sum + args[i] end
    return sum
  end
end
local f = makeadd(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)
print(f())
