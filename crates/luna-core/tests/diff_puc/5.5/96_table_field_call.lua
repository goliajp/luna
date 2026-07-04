-- v2.11 CORPUS-II: t:method() syntax vs t.method(t).
local t = {v = 10}
function t:get() return self.v end
function t:set(v) self.v = v; return self end
print(t:get())
print(t.get(t))       -- same
t:set(20):set(30)     -- chained
print(t.v)
