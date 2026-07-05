-- v2.12 CORPUS-III: ipairs walks integer keys 1..N.
local t = {10, 20, 30}
local sum = 0
for i, v in ipairs(t) do sum = sum + v end
print(sum)

-- 5.3+: ipairs consults __index; iteration ends at the first
-- index whose lookup yields nil (k=4 here).
local proxy = setmetatable({}, {
  __index = function(_, k)
    if k <= 3 then return k * 10 end
  end,
})
for i, v in ipairs(proxy) do
  io.write(i, "=", v, " ")
end
print("(end)")
