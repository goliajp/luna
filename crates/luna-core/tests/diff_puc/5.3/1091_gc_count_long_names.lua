-- names longer than forty characters, which 5.2 on makes long strings:
-- the ones a function keeps for its local variables and upvalues live as
-- long as its prototype, a name scanned again is garbage at once
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return collectgarbage("count") * 1024
end
local before = count()
local f = (loadstring or load)([[
local a_local_variable_with_a_name_longer_than_forty_chars = 1
local function g() return a_local_variable_with_a_name_longer_than_forty_chars end
local function h(a_parameter_with_a_name_longer_than_forty_chars)
  return a_parameter_with_a_name_longer_than_forty_chars
end
return g, h
]])
print("loaded", string.format("%.0f", count() - before))
local g, h = f()
f = nil
print("ran", string.format("%.0f", count() - before))
g, h = nil, nil
print("dropped", string.format("%.0f", count() - before))
