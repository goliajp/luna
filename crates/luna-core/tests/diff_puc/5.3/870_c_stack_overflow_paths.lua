-- Unbounded nesting through metamethods, library callbacks, coroutines
-- and protected calls ends in PUC's C-call-limit or Lua-stack error,
-- caught by pcall. Positions are cut down to "@" (chunk names differ).
local function norm(e) return ((tostring(e):gsub("^.*:%d+: ", "@ "))) end
local function try(name, f, ...) local ok, e = pcall(f, ...) print(name, ok, norm(e)) end
local mt = {}
mt.__index = function(t, k) return t[k] end
mt.__newindex = function(t, k, v) t[k] = v end
mt.__eq = function(a, b) return a == b end
mt.__lt = function(a, b) return a < b end
mt.__le = function(a, b) return a <= b end
mt.__add = function(a, b) return a + b end
mt.__concat = function(a, b) return a .. b end
mt.__unm = function(a) return -a end
mt.__tostring = function(a) return tostring(a) end
mt.__call = function(s) return (s()) end
local a, b = setmetatable({}, mt), setmetatable({}, mt)
try("index", function() return a.x end)
try("newindex", function() a.x = 1 end)
try("eq", function() return a == b end)
try("lt", function() return a < b end)
try("le", function() return a <= b end)
try("add", function() return a + 1 end)
try("concat", function() return a .. "x" end)
try("unm", function() return -a end)
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
mt.__len = function(x) return #x end
try("len", function() return #a end)
local p = setmetatable({}, {__pairs = function(t) return pairs(t) end})
try("pairs", function() for k in pairs(p) do end end)
