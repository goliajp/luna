-- v2.11 CORPUS-II: closure over tbc variable.
local function make()
  local x <close> = setmetatable({v=42}, {__close = function() end})
  return function() return x.v end
end
-- Note: closure escapes scope of tbc, but x is captured before close
-- fires. Lua semantic: x still valid to reference via closure after
-- the do-end, but if __close mutated it we'd see the change.
local f = make()
print(f())  -- 42
