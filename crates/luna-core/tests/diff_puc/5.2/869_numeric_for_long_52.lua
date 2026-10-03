-- Numeric for loops long enough for a JIT to take over: the values the
-- loop variable takes with float steps, zero steps, NaN and infinite
-- limits, near 2^53 and (5.3) past the integer range.
local fmt = function(x)
  if math.type and math.type(x) == "integer" then return tostring(x) end
  return string.format("%.17g", x)
end
local function run(name, f)
  local out, n = {}, 0
  f(function(i)
    n = n + 1
    out[#out + 1] = fmt(i)
    return n >= 300
  end)
  print(name, n, table.concat(out, ",", math.max(1, #out - 4)))
end
run("int up", function(k) for i = 1, 1000 do if k(i) then break end end end)
run("int down", function(k) for i = 900, 1, -3 do if k(i) then break end end end)
run("float acc", function(k) for i = 0, 25, 0.1 do if k(i) then break end end end)
run("float down", function(k) for i = 10, -10, -0.0625 do if k(i) then break end end end)
run("zero step", function(k) for i = 10, 1, 0 do if k(i) then break end end end)
run("zero step float", function(k) for i = 10.5, 1, 0 do if k(i) then break end end end)
run("zero step up", function(k) for i = 1, 10, 0 do if k(i) then break end end end)
run("zero step frac limit", function(k) for i = 1, 1.5, 0 do if k(i) then break end end end)
run("nan up", function(k) for i = 1, 0/0 do if k(i) then break end end end)
run("nan down", function(k) for i = 1, 0/0, -1 do if k(i) then break end end end)
run("nan zero", function(k) for i = 1, 0/0, 0 do if k(i) then break end end end)
run("inf", function(k) for i = 1, 1/0 do if k(i) then break end end end)
run("-inf zero", function(k) for i = 1, -1/0, 0 do if k(i) then break end end end)
run("2^53", function(k) for i = 2^53 - 250, 2^53 + 4 do if k(i) then break end end end)
run("string", function(k) for i = "1", "400", "2" do if k(i) then break end end end)
-- an initial value and step that are integers the VM keeps (lengths)
local one, zero = #"x", #""
run("len frac limit", function(k) for i = one, 1.5, zero do if k(i) then break end end end)
run("len nan", function(k) for i = one, 0/0, -one do if k(i) then break end end end)
run("len nan zero", function(k) for i = one, 0/0, zero do if k(i) then break end end end)
local t = {}
for i = 1, 400 do t[i] = i * 2 end
run("len", function(k) for i = #t, 1, -1 do if k(t[i]) then break end end end)
run("half keys", function(k)
  local h = {}
  for i = 0.5, 200, 0.5 do h[i] = i end
  for i = 0.5, 200, 0.5 do if k(h[i]) then break end end
end)
if math.maxinteger then
  run("wrap up", function(k) for i = math.maxinteger - 200, math.maxinteger do if k(i) then break end end end)
  run("wrap down", function(k) for i = math.mininteger + 200, math.mininteger, -1 do if k(i) then break end end end)
end
