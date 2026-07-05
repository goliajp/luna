-- v2.14 CV.3: an error inside resume surfaces as (false, msg) —
-- prefix present, message intact after harness normalization.
local co = coroutine.create(function() error("inner err", 0) end)
print(coroutine.resume(co))
local co2 = coroutine.create(function() local x = nil; return x.y end)
local ok, e = coroutine.resume(co2)
print(ok, e:match("attempt to index") ~= nil)
print(coroutine.status(co2))
