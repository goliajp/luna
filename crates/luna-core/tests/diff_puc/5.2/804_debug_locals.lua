-- v3.1 debug slice: debug.getlocal / setlocal argument checks and the
-- hidden locals of a for loop, whose names differ by version.
local function names(level)
  local out = {}
  local n = 1
  while true do
    local name = debug.getlocal(level + 1, n)
    if not name then break end
    if name:find("^%(for") or name == "i" or name == "k" or name == "v" then out[#out + 1] = name end
    n = n + 1
  end
  return table.concat(out, ",")
end
for i = 1, 1 do print(names(1)) end
for k, v in pairs({1}) do print(names(1)) end
local function params(a, b, ...) local c = 1 return c end
print(debug.getlocal(params, 1), debug.getlocal(params, 2), debug.getlocal(params, 3))
print(pcall(debug.getlocal, 50, 1))
print(pcall(debug.getlocal, "x", 1))
print(pcall(debug.getlocal, 1, "x"))
print(pcall(debug.getlocal))
print(pcall(debug.setlocal, 50, 1, 1))
print(pcall(debug.setlocal, 1, 1))
print(pcall(debug.setlocal, "x", 1, 1))
local function set()
  local p, q = 1, 2
  local a, b = debug.setlocal(1, 1, 10), debug.setlocal(1, 9, 0)
  return p, q, a, b
end
print(set())
