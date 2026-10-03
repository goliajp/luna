local obj = {n = 0}
function obj:step(i) if i == 700 then local t = nil; return t.boom end self.n = self.n + i return self.n end
local function run() for i = 1, 1000 do obj:step(i) end end
run()
