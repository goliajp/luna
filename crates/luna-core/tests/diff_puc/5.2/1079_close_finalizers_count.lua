-- rule: lstate.lua_close.1 lgc.luaC_freeallobjects.1
-- What the state's close runs: every pending finalizer, in PUC's order,
-- each seeing the count as the close leaves it, allocating, one raising
-- (its error goes nowhere); then nothing more is printed
local function obj(fin) return setmetatable({}, {__gc = fin}) end
local n = 0
keep = {}
for i = 1, 6 do
  keep[i] = obj(function()
    n = n + 1
    local t = {}
    for j = 1, 20 do t[j] = {j} end
    print("fin", i, n, collectgarbage("count") * 1024)
    if i == 4 then error("finalizer " .. i .. " fails") end
  end)
end
keep.str = string.rep("x", 1000)
print("end of chunk", collectgarbage("count") * 1024)
