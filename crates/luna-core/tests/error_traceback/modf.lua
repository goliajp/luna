local m = {}
function m.f() error("in mod") end
package.loaded.mymod = m
local function g() m.f() end
g()
