-- v3.1 bytecode: 5.5 loops keep three hidden slots; their variables and
-- body locals, captured by closures and seen by debug.getlocal.
local fns = {}
for i = 1, 3 do
  local sq = i * i
  fns[#fns + 1] = function() return i, sq end
  for k, v in pairs({key = i}) do
    fns[#fns + 1] = function() return k, v, sq end
  end
end
for _, f in ipairs(fns) do print(f()) end
for x = 1.0, 2.0, 0.5 do io.write(x, " ") end
print()
for i = 1, 4 do
  if i % 2 == 0 then goto continue end
  io.write(i, " ")
  ::continue::
end
print()
-- inside a function, so only this function's locals are listed
local function probe()
  for _, v in ipairs({7}) do
    local inner = v + 1
    local names = {}
    local n = 1
    while true do
      local name = debug.getlocal(1, n)
      if not name or name:find("temporary") then break end
      -- hidden loop slots are named per dialect; list the source's locals
      if name:sub(1, 1) ~= "(" then names[#names + 1] = name end
      n = n + 1
    end
    print(table.concat(names, ","), inner)
  end
end
probe()
