-- repeat ... until conditions built from and, or, not, ~= and comparisons:
-- how many times the body runs for every operand combination, a captured
-- body local, and the lines a line hook reports for multi-line conditions

local vals = { n = 5, nil, false, true, 0, "x" }

local function count(a, b, c)
  local r = ""
  local n
  n = 0 repeat n = n + 1 until n > 2 or a and b; r = r .. n
  n = 0 repeat n = n + 1 until n > 2 or (a or b) and not c; r = r .. n
  n = 0 repeat n = n + 1 until not (n < 3) or a ~= b; r = r .. n
  n = 0 repeat n = n + 1 until n >= 3 or not (a and b) or c; r = r .. n
  n = 0 repeat n = n + 1 until (a) and ((b) or not (c)) or n == 3; r = r .. n
  n = 0 repeat n = n + 1 until n > 1 and (a or c) or n > 4; r = r .. n
  n = 0 repeat n = n + 1 until true; r = r .. n
  n = 0 repeat n = n + 1 until n > 2 and true; r = r .. n
  return r
end

local out = {}
for i = 1, vals.n do
  for j = 1, vals.n do
    for k = 1, vals.n do
      out[#out + 1] = count(vals[i], vals[j], vals[k])
    end
  end
end
print(table.concat(out, " "))

-- a body local captured by a closure: every pass closes it before looping
local fs = {}
local i = 0
repeat
  i = i + 1
  local x = i * 10
  fs[#fs + 1] = function() return x end
until i >= 4 or x == 1000 and i < 0
local s = {}
for _, f in ipairs(fs) do s[#s + 1] = f() end
print(table.concat(s, " "))

local function lines_of(f, ...)
  local base = debug.getinfo(f, "S").linedefined
  local seen = {}
  debug.sethook(function(_, l) seen[#seen + 1] = l - base end, "l")
  f(...)
  debug.sethook()
  return table.concat(seen, " ")
end

local function multi(a, b, c)
  local x = 0
  repeat
    x = x + 1
  until x > 2 and
        a < b or x > 4
  repeat
    x = x + 1
  until a or
        x > 7
  repeat
    x = x + 1
  until not
        (x < 10)
  repeat
    local y = x
    x = x + 1
  until y ~=
        c
  return x
end
print(lines_of(multi, 1, 2, 3))
print(lines_of(multi, 3, 2, 1))
print(lines_of(multi, 0, 9, 13))
