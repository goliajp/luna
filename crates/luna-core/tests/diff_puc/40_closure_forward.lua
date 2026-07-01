-- v2.10 CORPUS: forward-declared local capture.
local resolver  -- forward decl
local function pick(n) return resolver(n) end
resolver = function(n) return n * 100 end
print(pick(3))  -- 300
resolver = function(n) return -n end
print(pick(3))  -- -3 (upvalue rebind visible)
