-- if / while conditions built from and, or, not, ~= and comparisons:
-- the branch taken for every operand combination, and the lines a line
-- hook reports while multi-line conditions run

local vals = { n = 5, nil, false, true, 0, "x" }

local function branches(a, b, c)
  local r = ""
  if a and b then r = r .. "1" else r = r .. "0" end
  if a or b then r = r .. "1" else r = r .. "0" end
  if not a then r = r .. "1" else r = r .. "0" end
  if not (a and b) or c then r = r .. "1" else r = r .. "0" end
  if a and (b or c) then r = r .. "1" else r = r .. "0" end
  if (a or b) and not c then r = r .. "1" else r = r .. "0" end
  if a ~= b then r = r .. "1" else r = r .. "0" end
  if not (a == b) and c then r = r .. "1" else r = r .. "0" end
  if a and true then r = r .. "1" else r = r .. "0" end
  if nil or a then r = r .. "1" else r = r .. "0" end
  if a and nil then r = r .. "1" else r = r .. "0" end
  if false or not b then r = r .. "1" else r = r .. "0" end
  if not not c then r = r .. "1" else r = r .. "0" end
  if (a) and ((b) or not (c)) then r = r .. "1" else r = r .. "0" end
  return r
end

local function values(a, b, c)
  local function f(x) return x and "1" or "0" end
  return f(a and b) .. f(a or b) .. f(not a) .. f(not (a and b) or c)
    .. f(a and (b or c)) .. f((a or b) and not c) .. f(a ~= b)
    .. f(not (a == b) and c) .. f(a and true) .. f(nil or a) .. f(a and nil)
    .. f(false or not b) .. f(not not c) .. f((a) and ((b) or not (c)))
end

local bad = 0
for i = 1, vals.n do
  for j = 1, vals.n do
    for k = 1, vals.n do
      local a, b, c = vals[i], vals[j], vals[k]
      local r, v = branches(a, b, c), values(a, b, c)
      if r ~= v then bad = bad + 1 end
      io.write(r, i == vals.n and j == vals.n and k == vals.n and "\n" or " ")
    end
  end
end
print("mismatches", bad)

local function numeric(a, b, c)
  local r = ""
  if a < b and b < c then r = r .. "1" else r = r .. "0" end
  if a <= b or b >= c then r = r .. "1" else r = r .. "0" end
  if not (a < b) and b ~= c then r = r .. "1" else r = r .. "0" end
  if a == 1 or b > 2 and c ~= 3 then r = r .. "1" else r = r .. "0" end
  if not (a > 1 or b <= 2) then r = r .. "1" else r = r .. "0" end
  return r
end
local nums = {}
for a = 0, 3 do
  for b = 0, 3 do
    for c = 0, 3 do nums[#nums + 1] = numeric(a, b, c) end
  end
end
print(table.concat(nums, " "))

local function loops(t, lim)
  local i, n, steps = 1, #t, 0
  while i <= n and t[i] < lim do i = i + 1 end
  local j = 0
  while not (j >= 4) and (t[j + 1] or lim) ~= lim do j = j + 1 end
  local k = 10
  while k > 0 and (k % 3 ~= 0 or k > 7) do k = k - 1; steps = steps + 1 end
  while true do
    steps = steps + 1
    if steps > 20 or steps % 7 == 0 then break end
  end
  return i, j, k, steps
end
print(loops({ 1, 2, 3, 9, 4 }, 5))
print(loops({ 5 }, 5))
print(loops({}, 0))

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
  if a < b and
     b < c
  then
    x = x + 1
  end
  if a and
     b
  then
    x = x + 2
  end
  if not
     c
  then
    x = x + 4
  end
  if a ~=
     b
  then
    x = x + 8
  end
  if a or
     b > c
  then
    x = x + 16
  end
  while a < c and
        b > 0
  do
    b = b - 1
  end
  return x
end
print(lines_of(multi, 1, 2, 3))
print(lines_of(multi, 3, 2, 1))
print(lines_of(multi, 1, 0, 0))
