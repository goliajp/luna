local limit = 800
local function check(i) if i > limit then error({code = i}) end return i end
local function run() local s = 0 for i = 1, 1000 do s = s + check(i) end return s end
run()
