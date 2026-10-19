-- 5.4 on, the parser's "C stack overflow" is a runtime error raised inside
-- the load's protected call: the message handler installed runs on it
-- while the parser's buffers are still there, and load returns what the
-- handler made of it
local deep = "return " .. ("("):rep(300) .. "1" .. (")"):rep(300)
local keep = {""}
for i = 1, 3000 do keep[#keep + 1] = "k" .. i end
local res = {}
local function run()
  for pass = 1, 2 do
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    local before = collectgarbage("count")
    local f, e = load(deep)
    local mid = collectgarbage("count")
    collectgarbage(); collectgarbage(); collectgarbage("stop")
    res[pass] = {f, e, mid - before, collectgarbage("count") - before}
  end
end
xpcall(run, function(m) return "handled: " .. tostring(m) end)
for _, r in ipairs(res) do
  print(type(r[1]), r[2], string.format("%.0f %.0f", r[3] * 1024, r[4] * 1024))
end
