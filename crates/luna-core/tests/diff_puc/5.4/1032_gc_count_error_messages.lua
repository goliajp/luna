-- errors of the interpreter and of the libraries, caught; their messages
-- are made and dropped
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local t = {}
local errs = {
  function() return 1 + {} end,
  function() return #nil end,
  function() return t.x.y end,
  function() return ("x")() end,
  function() return string.rep() end,
  function() return math.floor("a") end,
  function() error("plain") end,
  function() error({}) end,
  function() error("lvl", 2) end,
  function() return 1 < "x" end,
  function() return ("a") .. {} end,
}
local n = 0
for _, f in ipairs(errs) do
  local ok, e = pcall(f)
  if not ok then n = n + 1 end
end
print(n, count() - base)
