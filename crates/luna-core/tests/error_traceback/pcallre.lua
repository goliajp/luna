local ok, e = pcall(error, "first")
local function again() error(e, 0) end
again()
