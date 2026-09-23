-- v3.1 trace JIT: `t[k] = v` in a compiled loop stores under the key the
-- register holds, whatever its type (a string key once went in as the
-- integer of its pointer).
local src = {}
for i = 1, 50 do src["k" .. i] = i end
local dst = {}
for r = 1, 20 do
  for k, v in pairs(src) do dst[k] = v end
end
local n = 0
for _ in pairs(dst) do n = n + 1 end
print(n, dst.k7, dst.k50)

local f = {}
for i = 1, 300 do f[i + 0.5] = i; f[i * 1.0] = -i end
local m = 0
for _ in pairs(f) do m = m + 1 end
print(m, f[10.5], f[10], math.type(next({[2.0] = true})))

local counts = {}
local names = {"a", "b", "c"}
for i = 1, 300 do
  local k = names[i % 3 + 1]
  counts[k] = (counts[k] or 0) + 1
end
print(counts.a, counts.b, counts.c)

local ok, err = pcall(function()
  local t = {}
  for i = 1, 300 do
    local k = i
    if i == 250 then k = 0 / 0 end
    t[k] = i
  end
end)
print(ok, (err:gsub("^.-:%d+: ", "")))
