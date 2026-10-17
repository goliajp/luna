-- a full stack: the handler is called where the overflow was raised
local function rec() return 1 + rec() end
local ok, e = xpcall(rec, error)
print(ok, (string.gsub(tostring(e), "^.-:%d+: ", "")))
local function h(m) return (select(2, pcall(string.rep))) end
ok, e = xpcall(rec, h)
print(ok, (string.gsub(tostring(e), "^.-:%d+: ", "")))
local c = 0
local function h2(m) c = c + 1; local t = {}; return (h2(m)) end
ok, e = xpcall(rec, h2)
print(ok, (string.gsub(tostring(e), "^.-:%d+: ", "")), c > 0)
