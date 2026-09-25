-- v3.1 bytecode: generic-for closing values, to-be-closed locals in loops,
-- and loop variables captured by closures.
local order = {}
local function closer(name)
  return setmetatable({}, {__close = function() order[#order + 1] = name end})
end
local function iter(t)
  local i = 0
  return function() i = i + 1; return t[i] end, nil, nil, closer("iter")
end
local fns = {}
for v in iter({"a", "b"}) do
  local c <close> = closer("body" .. v)
  fns[#fns + 1] = function() return v end
end
print(table.concat(order, ","), fns[1](), fns[2]())
for i = 1, 3 do
  local d <close> = closer("n" .. i)
  if i == 2 then break end
end
print(table.concat(order, ","))
