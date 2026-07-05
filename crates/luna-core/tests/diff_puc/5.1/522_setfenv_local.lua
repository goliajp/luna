-- v2.15 P2.5 (5.1): setfenv on nested function.
local function f()
  return x    -- reads x from environment
end
setfenv(f, {x = "custom_env"})
print(f())
