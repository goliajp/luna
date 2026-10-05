-- Unbounded nesting through library callbacks, __tostring, __call,
-- coroutines, protected calls and message handlers ends in PUC's
-- C-call-limit or Lua-stack error, caught by pcall. Positions are cut
-- down to "@" (chunk names differ). Recursion through other metamethods
-- is left out: luna does not count those calls against the C-call limit
-- (docs/compatibility.md).
local function norm(e) return ((tostring(e):gsub("^.*:%d+: ", "@ "))) end
local function try(name, f, ...) local ok, e = pcall(f, ...) print(name, ok, norm(e)) end
local mt = {}
mt.__tostring = function(a) return tostring(a) end
mt.__call = function(s) return (s()) end
local a = setmetatable({}, mt)
try("tostring", tostring, a)
try("call", function() return a() end)
local function rec() return rec() + 1 end
try("lua", rec)
local function srt() table.sort({3, 2, 1}, function(x, y) srt() return x < y end) end
try("sort", srt)
local function gs() return (string.gsub("x", "x", function() return gs() end)) end
try("gsub", gs)
local function wr() return coroutine.wrap(wr)() end
try("wrap", wr)
local last
local function pc() local ok, e = pcall(pc) if not ok then last = e end end
pc()
print("pcall", norm(last))
local function h(m) return h(m) .. "" end
print("handler", xpcall(error, h))
