-- v2.11 CORPUS-II: __index function receives (t, k) args.
local mt = {__index = function(t, k)
  return "lazy_" .. k
end}
local o = setmetatable({}, mt)
print(o.x, o.foo, o.bar_baz)

-- __index function called only on missing
o.present = "explicit"
print(o.present)  -- "explicit" (not lazy)
