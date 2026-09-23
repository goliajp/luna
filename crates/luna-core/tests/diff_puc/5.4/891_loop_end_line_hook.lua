-- PUC emits a loop's closing code after reading its `end` and attributes
-- it to that line: the CLOSE of a generic for's closing value (5.4+) and
-- the CLOSE at a 5.4 `break` label. A line hook sees the `end` line once
-- as such a loop exits. Hook events are counted per line.
local fs = {}
local counts = {}
local function hook(_, line) counts[line] = (counts[line] or 0) + 1 end
local function run()
  for i = 1, 2 do
    fs[#fs + 1] = function() return i end
  end
  for _, v in ipairs({1, 2}) do
    fs[#fs + 1] = function() return v end
  end
  local n = 0
  while n < 2 do
    n = n + 1
    local w = n
    fs[#fs + 1] = function() return w end
  end
  repeat
    local r = n
    fs[#fs + 1] = function() return r end
    n = n - 1
  until n == 0
  do
    local d = 1
    fs[#fs + 1] = function() return d end
  end
  for i = 1, 2 do
    local plain = i
  end
  if n == 0 then
    local c = 3
    fs[#fs + 1] = function() return c end
  end
  for i = 1, 2 do
    local k = i
    fs[#fs + 1] = function() return k end
    if i == 2 then break end
  end
end
debug.sethook(hook, "l")
run()
debug.sethook()
local lines = {}
for l, c in pairs(counts) do lines[#lines + 1] = l end
table.sort(lines)
-- relative to run's first line: the diff harness prepends code on luna's
-- side, which shifts absolute line numbers
local base = debug.getinfo(run, "S").linedefined
for _, l in ipairs(lines) do io.write(l - base, ":", counts[l], " ") end
print()
