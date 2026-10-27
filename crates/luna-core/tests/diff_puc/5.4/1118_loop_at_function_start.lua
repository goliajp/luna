-- Functions whose first instruction is a loop's head, so the loop
-- branches back to where the function starts; called often enough for
-- the JIT to take them.
local fs = {
  function(a, b) while a < b do a = a + 1 if a == 5 then return a end end return 0 end,
  function(a, b) for i = a, b do if i == 1000 then return i end end return 0 end,
  function(a, b) while a < b do a = a + 1 if a == 5 then return end end end,
  function(a, b) repeat a = a + 1 if a == 5 then return a end until a >= b return -1 end,
  function(a, b) local s = 0 while a < b do a = a + 1 s = s + a if s > 9 then return s end end return s end,
}
for k, g in ipairs(fs) do
  local s = {}
  for i = 1, 300 do
    local n = select("#", g(i % 7, i % 5))
    local v = g(i % 7, i % 5)
    if i % 50 == 0 then s[#s + 1] = n .. ":" .. tostring(v) end
  end
  print(k, table.concat(s, " "))
end
