-- The sign of the NaNs arithmetic and the math library make (x86 makes
-- negative ones, aarch64 positive ones), and how each format writes them.
local zero, one, huge = 0, 1, math.huge
local fz = 0.0
local r = {}
local function add(name, x)
  r[#r + 1] = name .. "=" .. tostring(x) .. "|" .. string.format("%.17g|%g|%f|%e|%5.1f", x, x, x, x, x)
end
add("0/0 const", 0/0)
add("-(0/0) const", -(0/0))
add("0.0/0.0 const", 0.0/0.0)
add("-(0.0/0.0) const", -(0.0/0.0))
add("zero/zero", zero/zero)
add("fz/fz", fz/fz)
add("-(fz/fz)", -(fz/fz))
add("huge-huge", huge-huge)
add("-huge+huge", -huge+huge)
add("huge*0", huge*0)
add("0*-huge", 0*-huge)
add("huge/huge", huge/huge)
add("sqrt(-1)", math.sqrt(-1))
add("-sqrt(-1)", -math.sqrt(-1))
add("abs(fz/fz)", math.abs(fz/fz))
add("fmod(1,0.0)", math.fmod(1, 0.0))
add("fmod(huge,1)", math.fmod(huge, 1))
add("huge%1", huge % 1)
add("1%0.0", 1 % fz)
add("fz/fz+1", fz/fz + 1)
add("1-fz/fz", 1 - fz/fz)
add("(fz/fz)*-1", (fz/fz) * -1)
add("log(-1)", math.log(-1))
add("acos(2)", math.acos(2))
add("huge^0 x", (huge - huge) ^ 1)
add("tonumber", tonumber(tostring(fz/fz)) or 0/0)
local t = {}
for i = 1, 5000 do t[i] = (i - i) / (i - i) end
add("loop div", t[5000])
for i = 1, 5000 do t[i] = -((i - i) / (i - i)) end
add("loop neg", t[5000])
for i = 1, 5000 do t[i] = huge * (i - i) end
add("loop mul", t[5000])
for i = 1, 5000 do t[i] = 0/0 end
add("loop const div", t[5000])
for i = 1, 5000 do t[i] = -(0/0) end
add("loop const neg", t[5000])
for i = 1, 5000 do local z = fz * i t[i] = z / z - huge end
add("loop local div", t[5000])
for i = 1, 5000 do t[i] = huge - huge end
add("loop huge-huge", t[5000])
for i = 1, 5000 do t[i] = math.abs(-(fz/fz)) end
add("loop abs", t[5000])
for i = 1, 5000 do t[i] = (fz/fz) % 2 end
add("loop mod", t[5000])
local function f(a, b) return a / b end
for i = 1, 5000 do t[i] = f(fz, fz) end
add("call div", t[5000])
-- no %a here: PUC 5.2 has it only when built with LUA_USE_AFORMAT (make
-- linux, not make posix), while luna 5.2 always has it
print(table.concat(r, "\n"))
