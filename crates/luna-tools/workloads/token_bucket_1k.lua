-- v2.4 Phase Soak fixture: 1k-token-bucket churn.
--
-- Simulates a rate-limiter checking 1000 buckets per iteration —
-- the same workload shape v1.x → v2.x perf attacks measured
-- against LuaJIT (charter floor 1.18× per `v2.1-perf-absolute-
-- exhaustion.md` §). Stresses table.get / table.set hot path +
-- GC of intermediate values.

local buckets = {}
for i = 1, 1000 do
  buckets[i] = {tokens = 100, refill_rate = 1.0}
end

local now = 0
for i = 1, 1000 do
  local b = buckets[i]
  -- refill
  b.tokens = math.min(100, b.tokens + b.refill_rate)
  -- consume 1 token if available
  if b.tokens >= 1 then
    b.tokens = b.tokens - 1
  end
  now = now + 1
end
