-- 5.1 codes no LOADNIL for a local at the very start of a function, its
-- register already nil; once a block has ended there, at the same pc, that
-- pc is a jump target and the local's LOADNIL is coded: its line runs
local function f()
  do local a end
  do local b end
  do local c end
  local d
  return d
end
local lines = {}
debug.sethook(function(_, l) lines[#lines + 1] = l end, "l")
f()
debug.sethook()
print(table.concat(lines, " "))
