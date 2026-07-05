-- v2.15 P2.5 (5.5): deep table copy via recursion.
local function deepcopy(t, seen)
  if type(t) ~= "table" then return t end
  seen = seen or {}
  if seen[t] then return seen[t] end
  local r = {}
  seen[t] = r
  for k, v in pairs(t) do r[deepcopy(k, seen)] = deepcopy(v, seen) end
  return r
end
local orig = {a = 1, nested = {b = 2, c = {3, 4, 5}}}
local copy = deepcopy(orig)
copy.a = 99
copy.nested.c[1] = 99
print(orig.a, copy.a)
print(orig.nested.c[1], copy.nested.c[1])
