-- v3.1 debug slice: debug.getinfo fills only the fields of the requested
-- options, and rejects letters its version does not know ('t' is 5.2+, 'r'
-- 5.4+) — after the level is resolved, so an absent level is still nil.
local function keys(t)
  if type(t) ~= "table" then return tostring(t) end
  local ks = {}
  for k in pairs(t) do ks[#ks + 1] = k end
  table.sort(ks)
  return table.concat(ks, ",")
end
local function f(a, ...) return a end
for _, o in ipairs{"S", "l", "u", "n", "t", "r", "L", "f", "", "q"} do
  print(o, keys(select(2, pcall(debug.getinfo, f, o))),
    keys(select(2, pcall(debug.getinfo, 1, o))),
    keys(select(2, pcall(debug.getinfo, math.abs, o))))
end
print(debug.getinfo(100, "q"))
print(pcall(debug.getinfo, {}))
print(pcall(debug.getinfo, 1, {}))
