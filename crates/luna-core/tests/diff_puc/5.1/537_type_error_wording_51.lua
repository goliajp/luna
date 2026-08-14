-- v2.18 P3: PUC <=5.2 words type errors operand-first --
-- "attempt to call field 'f' (a nil value)". 5.3 flipped to type-first.
-- luna emitted the 5.3+ form on every dialect, so every such error was
-- worded wrong under 5.1/5.2.
--
-- Only the message body is compared: pcall's message carries a chunkname
-- and line that the harness wraps differently on each side.
local function m(f)
  local _, e = pcall(f)
  print((tostring(e):gsub("^[^:]*:%d+: ", "")))
end
local t = {}
m(function() t.missing() end)
m(function() t.a.b() end)
local up
m(function() up() end)
m(function() undefinedglobal() end)
m(function() local s = "x" s:nope() end)
m(function() local n = 5 n() end)
m(function() local a = nil return a.b end)
-- no operand name: identical across dialects
m(function() local g = function() return nil end g()() end)
