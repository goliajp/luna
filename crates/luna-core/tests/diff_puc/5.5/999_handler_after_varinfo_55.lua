-- where a message handler runs: an error naming its operand ("local 'x1'")
-- leaves that name on the stack first in 5.3+, so the handler starts one
-- slot higher; the depth a recursion reaches inside it shows the slot,
-- printed against the first error's so that the harness's own frames
-- below do not count
local load = loadstring or load
local function probe()
  local d = 0
  local function f() d = d + 1 return f() + 1 end
  pcall(f)
  return d
end
local h = function(m) return probe() end
local base
for k = 1, 4 do
  local names = {}
  for i = 1, k do names[i] = "x" .. i end
  local decl = "local " .. table.concat(names, ",")
  for _, case in ipairs({{"index", " return x1.y"}, {"arith", " return x1 + 1"},
                         {"call", " return x1()"}, {"bare", " return (x1 or nil) + 1"}}) do
    local d = select(2, xpcall(load(decl .. case[2]), h))
    base = base or d
    print(k, case[1], d - base)
  end
end
