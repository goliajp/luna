-- v2.12 CORPUS-III: coroutine.wrap propagates error.
local w = coroutine.wrap(function()
  coroutine.yield(1)
  error("in-wrap")
end)
print(w())   -- 1
local ok, err = pcall(w)
print(ok, err:match(": (.+)$") or err)
