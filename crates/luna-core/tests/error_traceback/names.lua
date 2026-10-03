local t = {}
function t.field(x) if x == 1 then error("field") end return x end
function t:method(x) return t.field(x) end
gfun = function(x) return t:method(x) end
local function loc(x) local r = gfun(x); return r end
local up = function(x) local r = loc(x); return r end
local function viaup(x) local r = up(x); return r end
local function tail(x) return viaup(x) end
local r = tail(1)
