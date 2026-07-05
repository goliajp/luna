-- v2.15 P2.5: coroutine.close returns false + error on dead-error state.
local function strip(e) return type(e) == "string" and (e:match(": (.+)$") or e) or tostring(e) end
local co = coroutine.create(function() error("crash") end)
coroutine.resume(co)                -- consumes error → dead
print(coroutine.status(co))         -- dead
-- close on already-dead-error coroutine
local ok, err = coroutine.close(co)
print(ok, strip(err))
