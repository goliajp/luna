-- a loaded function keeps the strings of its local variable and upvalue
-- names: the count after a full collection still has them
local src = [[
local alpha_name_one, beta_name_two, gamma_name_three = 1, 2, 3
local function inner_func_four(delta_param_five)
  local epsilon_local_six = delta_param_five + alpha_name_one
  return epsilon_local_six
end
return inner_func_four
]]
local ld = loadstring or load
collectgarbage(); collectgarbage()
local before = collectgarbage("count")
local f = ld(src)
collectgarbage(); collectgarbage()
local after = collectgarbage("count")
print(f ~= nil, string.format("%.0f", (after - before) * 1024))
