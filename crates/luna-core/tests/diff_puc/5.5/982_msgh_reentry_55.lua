-- An error raised inside an xpcall message handler runs the handler again
-- at that point (luaG_errormsg keeps L->errfunc set), so a traceback taken
-- by the inner run still shows the outer run's frames. Line numbers and the
-- file name are masked: the diff harness prepends code on luna's side.
local function mask(s) return (s:gsub("[%w_./-]*%.lua", "F"):gsub(":%d+:", ":N:"):gsub(":%d+>", ":N>")) end
local function h(m)
  if tostring(m):find("inner") then return debug.traceback("H:" .. tostring(m), 1) end
  error("inner", 0)
end
local function f() error("outer", 0) end
local ok, e = xpcall(f, h)
-- the levels below xpcall differ with how the chunk was started
e = e:match("^(.-%[C%]: in [%w ]*'?xpcall'?)")
print(ok, mask(e))
-- a handler that always fails
local n = 0
local ok2, e2 = xpcall(f, function(m) n = n + 1 error("again", 0) end)
print(ok2, e2, n > 100)
-- the inner run's result is what the outer run throws
print(xpcall(f, function(m) if m == "outer" then error({}) end return "T:" .. type(m) end))
