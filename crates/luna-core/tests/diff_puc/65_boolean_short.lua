-- v2.11 CORPUS-II: short-circuit evaluation with side effects.
local order = {}
local function tag(name, v)
  order[#order+1] = name
  return v
end
tag("a", true) and tag("b", false) or tag("c", "res")
print(table.concat(order, ","))    -- a,b,c

order = {}
tag("x", false) and tag("y", true) or tag("z", "res")
print(table.concat(order, ","))    -- x,z (skip y because a is false)
