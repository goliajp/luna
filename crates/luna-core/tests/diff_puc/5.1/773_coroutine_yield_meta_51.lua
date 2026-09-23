-- v3.1 iopkg: 5.1 calls metamethods and generic-for iterators through
-- luaD_call, a C level, so a coroutine cannot yield from inside one; a
-- __call target is an ordinary call and may yield. 5.2 lifted the rule.
local t = setmetatable({}, {
  __index = function(_, k) coroutine.yield(k) return 1 end,
  __add = function() coroutine.yield("add") return 2 end,
  __call = function() coroutine.yield("call") return 3 end,
  __lt = function() coroutine.yield("lt") return true end,
  __concat = function() coroutine.yield("cat") return "c" end,
})
local cases = {
  {"index", function() return t.x end},
  {"add", function() return t + 1 end},
  {"call", function() return t() end},
  {"lt", function() return t < t end},
  {"concat", function() return t .. "x" end},
  {"iterator", function() for i in function() coroutine.yield("iter") end do end return "for" end},
  {"inside pcall", function() return select(2, pcall(coroutine.yield, "p")) end},
}
for _, c in ipairs(cases) do
  local co = coroutine.create(c[2])
  local a, b = coroutine.resume(co)
  local d, e = coroutine.resume(co, "back")
  print(c[1], a, b, d, e)
end
