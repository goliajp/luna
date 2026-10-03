-- Fixture for `luna-soak --vm-churn`: each Vm runs this until the method
-- JIT has compiled `mix` and the trace JIT has compiled a loop, then the
-- Vm is dropped.

local function mix(a, b)
  return a + b
end

local function refill(b, rate)
  local t = b.tokens + rate
  if t > 100 then t = 100 end
  b.tokens = t
  return t
end

local buckets = {}
for i = 1, 200 do
  buckets[i] = {tokens = i % 100, name = "bucket" .. i}
end

local h, total = 0, 0
for round = 1, 5 do
  for i = 1, 200 do
    h = mix(h, i)
    total = total + refill(buckets[i], 1.5)
  end
end
return h, total
