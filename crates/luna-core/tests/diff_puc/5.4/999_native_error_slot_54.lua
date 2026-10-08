local mt_err = "return setmetatable({}, {__index = function() error('in __index') end, " ..
  "__len = function() error('in __len') end, __lt = function() error('in __lt') end, " ..
  "__le = function() error('in __le') end, __tostring = function() error('in __tostring') end, " ..
  "__call = function() error('in __call') end, __eq = function() error('in __eq') end, " ..
  "__concat = function() error('in __concat') end, __pairs = function() error('in __pairs') end})"
local named = "return setmetatable({}, {__name = 'MyType'})"
local bigstr = "string.rep('x', 1100000)"

local cases = {
  { "assert false msg", "return assert, false, 'msg'" },
  { "error level 2", "return error, 'msg', 2" },
  { "error number level 0", "return error, 42, 0" },
  { "select 0", "return select, 0" },
  { "tonumber base 99", "return tonumber, '10', 99" },
  { "setmetatable protected", "return setmetatable, setmetatable({}, {__metatable = 1}), {}" },
  { "setmetatable bad mt", "return setmetatable, {}, 1" },
  { "rawlen number", "return rawlen, 1" },
  { "next bad key", "return next, {}, 'nokey'" },
  { "pairs __pairs raising", "return pairs, (" .. mt_err:sub(8) .. ")" },
  { "load bad chunkname", "return load, 'x', {}" },
  { "tostring __tostring raising", "return tostring, (" .. mt_err:sub(8) .. ")" },
  { "tostring __tostring not string", "return tostring, setmetatable({}, {__tostring = function() return {} end})" },
  { "collectgarbage bad option", "return collectgarbage, 'bad'" },
  { "typeerror __name", "return string.rep, " .. named:sub(8) },
  { "argerror method self", "return function() local s = setmetatable({}, {__index = {f = string.rep}}) return s:f() end" },
  { "argerror local name", "return function() local f = string.rep return f() end" },
  { "argerror global name", "return function() return string.rep() end" },
  { "argerror upvalue name", "local f = string.rep return function() return f() end" },
  { "string.rep too large", "if _VERSION == 'Lua 5.1' then return end return string.rep, 'xx', 2^62" },
  { "string.char 256", "return string.char, 256" },
  { "string.format no value", "return string.format, '%d'" },
  { "string.format s __tostring raising", "return string.format, '%s', (" .. mt_err:sub(8) .. ")" },
  { "string.find malformed %", "return string.find, 'x', '%'" },
  { "string.gsub repl raising", "return string.gsub, 'x', 'x', function() error('in repl') end" },
  { "string.gsub repl table __index raising", "return string.gsub, 'x', 'x', (" .. mt_err:sub(8) .. ")" },
  { "string.pack bad option", "return string.pack, 'y', 1" },
  { "string.unpack too short", "return string.unpack, 'i4', 'x'" },
  { "table.insert position", "return table.insert, {}, 5, 1" },
  { "table.insert __len raising", "return table.insert, (" .. mt_err:sub(8) .. "), 1" },
  { "table.concat invalid value", "return table.concat, {{}}" },
  { "table.concat __index raising", "return table.concat, (" .. mt_err:sub(8) .. "), '', 1, 2" },
  { "table.unpack too many", "return table.unpack, {}, 1, 1e8" },
  { "table.sort invalid order", "return table.sort, {5, 4, 3, 2, 1, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16}, function() return true end" },
  { "table.sort comparator raising", "return table.sort, {3, 1, 2}, function() error('in cmp') end" },
  { "math.random empty 2", "return math.random, 3, 1" },
  { "math.max __lt raising", "return math.max, (" .. mt_err:sub(8) .. "), (" .. mt_err:sub(8) .. ")" },
  { "os.date bad conversion", "return os.date, '%Ez'" },
  { "os.time missing field", "return os.time, {}" },
  { "file seek bad", "local f = io.tmpfile() return f.seek, f, 'bad'" },
  { "coroutine.wrap raising string", "return coroutine.wrap(function() error('in coroutine') end)" },
  { "coroutine.wrap dead", "return function() local f = coroutine.wrap(function() end) f() f() end" },
  { "coroutine.resume bad", "return coroutine.resume, 1" },
  { "debug.getinfo invalid option", "return debug.getinfo, 1, 'q'" },
  { "debug.getlocal level", "return debug.getlocal, 100, 1" },
  { "debug.upvaluejoin invalid", "return debug.upvaluejoin, function() end, 1, function() end, 1" },
  { "utf8.char out of range", "return utf8.char, -1" },
  { "require missing", "return require, 'no_such_module_r31f'" },
  { "ipairs iterator __index raising", "return ipairs({}), setmetatable({}, {__index = function() error('in __index') end}), 0" },
  { "coroutine.close main thread", "return coroutine.close, (coroutine.running())" },
  { "debug.sethook hook raising", "return function() debug.sethook(function() debug.sethook() error('in hook') end, 'c') local x = type(1) return x end" },
  { "searcher preload bad", "return (package.searchers or package.loaders)[1], {}" },
  { "require loader raising", "package.preload.r31f_m = function() package.preload.r31f_m = nil error('in loader') end return require, 'r31f_m'" },
  { "string arith idiv zero", "return function() return '1' // '0' end" },
  { "table.move __eq raising", "return table.move, setmetatable({}, {__eq = function() error('in __eq') end}), 1, 2, 2, {}" },
  { "os.time __newindex", "return os.time, setmetatable({year = 2000, month = 1, day = 1}, {__newindex = function() error('in __newindex') end})" },
  { "io.popen bad mode", "if _VERSION < 'Lua 5.3' then return end return io.popen, 'x', 'z'" },
  { "file read -1", "local f = io.tmpfile() return f.read, f, -1" },
}

-- the slot the xpcall message handler of each error below runs at, as
-- the number of stack slots `lua_checkstack` still grants inside it,
-- printed against the same count for a Lua error raised from this harness
-- (so the harness's own frames below do not count). The calls are made
-- from a tail-recursive walk, not from a loop body.
local unpack = table.unpack or unpack
local load = loadstring or load
-- PUC's collector shrinks a stack that is handling an error back from the
-- extra error space, at whatever allocation it runs; luna's does not
collectgarbage("stop")
local E = {}
local function fits(n) return (pcall(unpack, E, 1, n)) end
local guess = 999950
local free
-- one count costs a single call that succeeds: the handler slots are
-- close to one another, so the search starts at the last count
local function hfree(m)
  local n = guess
  if fits(n) then
    while fits(n + 1) do n = n + 1 end
  else
    repeat n = n - 1 until fits(n)
  end
  free, guess = n, n
  return m
end

local function pack(...) return { n = select("#", ...), ... } end

local function run(fargs)
  free = nil
  xpcall(fargs[1], hfree, unpack(fargs, 2, fargs.n))
  return free
end

local base = run(pack(function() local x x.y = 1 end))

local function one(label, code)
  local chunk = load(code)
  local okc, fargs = false, nil
  if chunk then
    okc, fargs = pcall(function() return pack(chunk()) end)
  end
  if not okc or type(fargs[1]) ~= "function" then
    print(label, "skip")
    return
  end
  if pcall(fargs[1], unpack(fargs, 2, fargs.n)) then
    print(label, "noerror")
    return
  end
  local f = run(pack(chunk()))
  print(label, f and f - base)
end

local function walk(list, i)
  if i > #list then return end
  one(list[i][1], list[i][2])
  return walk(list, i + 1)
end
walk(cases, 1)

-- a few library functions with the argument shapes of the natives corpus
local shapes = {
  { "setmetatable()", "return setmetatable" },
  { "string.rep({})", "return string.rep, {}" },
  { "math.floor(mt_err())", "local mt_err = function() " .. mt_err .. " end return math.floor, mt_err()" },
  { "table.concat(mt_err())", "local mt_err = function() " .. mt_err .. " end return table.concat, mt_err()" },
  { "tostring(mt_err())", "local mt_err = function() " .. mt_err .. " end return tostring, mt_err()" },
  { "os.time(mt_err())", "local mt_err = function() " .. mt_err .. " end return os.time, mt_err()" },
  { "pairs(mt_err())", "local mt_err = function() " .. mt_err .. " end return pairs, mt_err()" },
  { "string.format({})", "return string.format, {}" },
}
walk(shapes, 1)
