local function a() error("tail err") end
local function b() return a() end
local function c() return b() end
c()
