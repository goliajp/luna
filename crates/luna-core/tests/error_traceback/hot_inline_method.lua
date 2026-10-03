local P = {}
P.__index = P
function P.new(v) return setmetatable({v = v}, P) end
function P:get() if self.v == 950 then error("v950") end return self.v end
function P:twice() return self:get() * 2 end
local s = 0
for i = 1, 1000 do s = s + P.new(i):twice() end
