-- The hidden control variables of both for loops, as debug.getlocal
-- lists them ahead of the loop variable: their names and count are per
-- dialect.
local function locals(stop)
  local names = {}
  local i = 1
  while true do
    local name = debug.getlocal(2, i)
    names[#names + 1] = name
    if name == nil or name == stop then break end
    i = i + 1
  end
  return table.concat(names, ",")
end
local function loops()
  for i = 1, 1 do print(locals("i")) end
  for k, v in pairs({1}) do print(locals("k")) end
end
loops()
