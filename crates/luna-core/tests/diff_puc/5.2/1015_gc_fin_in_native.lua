-- rule: lgc.GCTM.12
-- a finalizer that runs while print is inside its call to tostring must
-- not change what that call returns
local function mk(f)
  if newproxy then local o = newproxy(true); getmetatable(o).__gc = f; return o end
  return setmetatable({}, {__gc = f})
end
do local x = mk(function() return 1, 2, 3 end) end
local t = {}
for i = 1, 200000 do t[i % 100] = {} end
print(1, nil, "x")
print(2, nil, "y")
