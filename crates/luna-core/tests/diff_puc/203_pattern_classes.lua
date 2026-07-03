-- v2.13 CORPUS-IV: pattern class matrix %a %d %s %p %x %c %w
-- and their complements.
local s = "aB3 .\t_x"
local function collect(pat)
  local out = {}
  for c in s:gmatch(pat) do out[#out + 1] = c end
  return table.concat(out, "")
end
print(collect("%a"), collect("%A"))
print(collect("%d"), collect("%w"))
print(collect("%s") == " \t", collect("%p"))
print(collect("%x"))
print(("abc"):match("^%l+$"), ("ABC"):match("^%u+$"))
