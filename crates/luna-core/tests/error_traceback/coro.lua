local co = coroutine.wrap(function() local function inner() error("in coro") end inner() end)
co()
