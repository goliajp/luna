-- v3.1 bytecode: every function carries its own environment, even one that
-- touches no global, and a closure it creates inherits it.
local function outer()
  return function() return marker end
end
setfenv(outer, {marker = "custom"})
print(outer()())
marker = "global"
print(outer()(), (function() return marker end)())
