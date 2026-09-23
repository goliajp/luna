-- v3.1 bytecode: generic-for variables captured by closures, nested loops,
-- break, and the loop's locals as debug.getlocal sees them.
local fns = {}
for i, v in ipairs({"a", "b", "c"}) do
  local w = v .. i
  fns[#fns + 1] = function() return i, v, w end
  if i == 2 then
    for k, x in pairs({z = 26}) do
      fns[#fns + 1] = function() return k, x, v end
    end
  end
end
for _, f in ipairs(fns) do print(f()) end
for n = 1, 3 do
  for _, s in ipairs({"x", "y"}) do
    if n == 2 then break end
    io.write(n, s, " ")
  end
end
print()
-- inside a function, so only this function's locals are listed
local function probe()
  for i, v in ipairs({10}) do
    local inner = v * 2
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
