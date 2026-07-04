-- v2.13 CORPUS-IV: generic-for with hand-written stateless and
-- stateful iterators + control-variable semantics.
local function range(n)
  return function(limit, i)
    i = i + 1
    if i <= limit then return i, i * i end
  end, n, 0
end
for i, sq in range(4) do io.write(i, ":", sq, " ") end
print()
-- 5.5: the generic-for control variable is <const> — assigning
-- to it is a COMPILE error (5.4 allowed it).
local f, err = load("for i in pairs({}) do i = 1 end")
print(f == nil, err ~= nil and err:find("const") ~= nil)
-- copying it out is fine
local out = {}
for i in range(3) do
  local captured = i
  out[#out + 1] = captured
end
print(table.concat(out, ","))
