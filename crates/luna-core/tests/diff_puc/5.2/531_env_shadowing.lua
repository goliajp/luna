-- v2.15 P2.5 (5.2): _ENV shadowing in nested scope (top-level
-- shadowing breaks the diff harness's stdout capture, so scope
-- to a function body only).
local function shadowed()
  local _ENV = {}
  _ENV.x = 42
  return _ENV.x, _ENV.foo    -- 42, nil
end
print(shadowed())
