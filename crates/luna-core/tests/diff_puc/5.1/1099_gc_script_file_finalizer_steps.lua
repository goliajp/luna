-- run as a script file
-- the output follows where the automatic collection steps run
-- 5.1 runs the script from lua.c's pmain, a C closure on the stack's
-- first slot, which the collector traverses; each step's work, and so
-- where the steps that run the raising finalizers fall, depends on it
local f = function() end
local i
local tb = debug.traceback
local function wrapf() return coroutine.wrap(f) end
local function step()
  local p = newproxy(true)
  getmetatable(p).__gc = function() error("gc") end
  p = nil
  local ok, e = xpcall(wrapf, tb)
  if not ok then io.write("inner ", i, "\n", e, "\n") end
end
for k = 1, 80 do
  i = k
  local ok, e = xpcall(step, tb)
  if not ok then io.write("outer ", i, "\n", e, "\n") end
end
