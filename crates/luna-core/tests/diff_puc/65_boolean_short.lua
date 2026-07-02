-- v2.11 CORPUS-II: short-circuit evaluation with side effects.
-- (An and/or chain is an expression, not a statement — bind it.)
local order = {}
local function tag(name, v)
  order[#order+1] = name
  return v
end
local r1 = tag("a", true) and tag("b", false) or tag("c", "res")
print(r1, table.concat(order, ","))    -- res  a,b,c

order = {}
local r2 = tag("x", false) and tag("y", true) or tag("z", "res")
print(r2, table.concat(order, ","))    -- res  x,z (y skipped)
