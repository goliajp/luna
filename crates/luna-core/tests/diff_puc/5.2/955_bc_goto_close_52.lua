-- v3.1 bytecode: a comparison guarding a jump that also closes upvalues
-- (break / goto out of a scope whose locals were captured).
local fns = {}
local i = 1
while true do
  local x = i * 10
  fns[#fns + 1] = function() return x end
  if i >= 3 then break end
  i = i + 1
end
for _, f in ipairs(fns) do io.write(f(), " ") end
print()
local got = {}
for n = 1, 4 do
  local y = n
  got[#got + 1] = function() return y end
  if n == 2 then goto done end
end
::done::
for _, f in ipairs(got) do io.write(f(), " ") end
print()
for _, v in ipairs({1, 2, 3}) do
  local c = v
  got[v] = function() return c end
  if v ~= 2 then goto continue end
  print("two", got[v]())
  ::continue::
end
