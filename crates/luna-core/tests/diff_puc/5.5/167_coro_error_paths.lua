-- v2.13 CORPUS-IV: error propagation — resume returns false+msg,
-- wrap re-raises catchable by pcall.
local function boom() error("boom", 0) end
local co = coroutine.create(boom)
local ok, msg = coroutine.resume(co)
print(ok, msg, coroutine.status(co))
local w = coroutine.wrap(boom)
local ok2, msg2 = pcall(w)
print(ok2, msg2)
-- resume on dead coroutine
local ok3, msg3 = coroutine.resume(co)
print(ok3, msg3)
