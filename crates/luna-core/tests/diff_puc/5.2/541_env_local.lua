-- v2.15 P2.5 (5.2): _ENV as first upvalue (nested).
local function f()
  return _ENV.print ~= nil     -- _ENV.print visible
end
print(f())
