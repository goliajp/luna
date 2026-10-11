-- numeric `for` preparation and stepping over the kinds of control values:
-- integer and float loops, empty ones, negative and fractional steps, loops
-- near the integer range's ends, NaN and infinite bounds, strings and a zero
-- step (errors), and short loops entered many times
local function msg(e) return (tostring(e):gsub("^[^:]*:%d+: ", "")) end
local function run(name, f)
  local ok, r = pcall(f)
  print(name, ok and r or "error: " .. msg(r))
end
local function collect(a, b, c)
  local t, n = {}, 0
  if c == nil then
    for i = a, b do n = n + 1; if n <= 6 then t[#t + 1] = tostring(i) end end
  else
    for i = a, b, c do n = n + 1; if n <= 6 then t[#t + 1] = tostring(i) end end
  end
  return n .. ": " .. table.concat(t, " ")
end

run("int up", function() return collect(1, 5) end)
run("int up step", function() return collect(1, 10, 3) end)
run("int down", function() return collect(5, 1, -1) end)
run("int down step", function() return collect(10, -10, -7) end)
run("int empty", function() return collect(5, 1) end)
run("int empty down", function() return collect(1, 5, -1) end)
run("int one", function() return collect(3, 3) end)
run("float up", function() return collect(0.5, 3) end)
run("float step", function() return collect(1, 2, 0.25) end)
run("float down", function() return collect(2.5, 0, -0.5) end)
run("float empty", function() return collect(2.5, 1) end)
run("float limit", function() return collect(1, 3.5) end)
run("float limit down", function() return collect(3, 0.5, -1) end)
run("float start", function() return collect(1.0, 3) end)
run("float step one", function() return collect(1, 3, 1.0) end)
run("big float limit", function() return collect(1, 1e300, 1e299) end)
run("nan limit", function() return collect(1, 0/0) end)
run("nan start", function() return collect(0/0, 1) end)
run("nan step", function() return collect(1, 3, 0/0) end)
run("neg zero step", function() return collect(1, 3, -0.0) end)
run("zero step", function() return collect(1, 3, 0) end)
run("zero float step", function() return collect(1, 3, 0.0) end)
run("string start", function() return collect("1", 3) end)
run("string limit", function() return collect(1, "3") end)
run("string step", function() return collect(1, 3, "1") end)
run("table limit", function() return collect(1, {}) end)
run("nil step", function() return collect(1, 3, false) end)

-- 5.3 wraps around at the ends of the integer range and loops forever
if _VERSION ~= "Lua 5.1" and _VERSION ~= "Lua 5.2" and _VERSION ~= "Lua 5.3" then
  local maxi, mini, int = math.maxinteger, math.mininteger, math.tointeger
  run("to maxinteger", function() return collect(maxi - 2, maxi) end)
  run("from mininteger", function() return collect(mini + 2, mini, -1) end)
  run("whole range step", function() return collect(mini, maxi, maxi) end)
  run("big step", function() return collect(0, maxi, int(2^62)) end)
  run("big negative step", function() return collect(0, mini, mini) end)
  run("float limit past max", function() return collect(maxi - 1, 2^63) end)
  run("float limit past min", function() return collect(mini + 1, -2^64, -1) end)
  run("float limit below start", function() return collect(maxi, 1.5) end)
  run("step mininteger", function() return collect(maxi, mini, mini) end)
  run("inf limit", function() return collect(1, 1/0, int(2^62)) end)
  run("-inf limit", function() return collect(1, -1/0, -int(2^62)) end)
  run("nan limit int", function() return collect(1, 0/0, 1) end)
end

-- short loops entered many times, integer and float
local s, f = 0, 0
for i = 1, 20000 do
  for j = 1, 3 do s = s + j end
  for j = i, i - 1 do s = s + 1000 end
  for x = 0.5, 2 do f = f + x end
  for j = 3, 1, -1 do s = s - j end
end
print("short", s, f)

-- the control variable is a copy: assigning to it does not change the loop
-- (5.5 refuses to compile the assignment)
local copy = (loadstring or load)([[
  local seen = {}
  for i = 1, 4 do seen[#seen + 1] = i; i = i * 10 end
  return table.concat(seen, " ")
]])
print("copy", copy and copy() or "read-only")
