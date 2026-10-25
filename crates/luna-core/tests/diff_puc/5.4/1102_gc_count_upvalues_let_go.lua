-- what the collector counts after each step that lets an upvalue go: 5.3's
-- upvalues are no collectable objects, each counting the closures that
-- hold it, and go with the last of them or as they close with none
-- holding them; the other dialects collect them as objects
collectgarbage()
collectgarbage("stop")
local function count() return math.floor(collectgarbage("count") * 1024) end
local out = {}
local function note(what, base)
  out[#out + 1] = what .. " " .. (count() - base)
end
-- a variable whose closures all died while it is open, then closed
local base = count()
local function a()
  local x = {}
  do local f = function() return x end end
  collectgarbage()
  note("open, unheld", base)
end
a()
note("closed", base)
-- upvaluejoin: the replaced upvalue goes with its last closure
local function mk() local v = {} return function() return v end end
local f1, f2 = mk(), mk()
base = count()
debug.upvaluejoin(f1, 1, f2, 1)
collectgarbage()
note("joined", base)
-- setupvalue changes the value, not the upvalue
base = count()
debug.setupvalue(f1, 1, "s")
collectgarbage()
note("set", base, f1() == "s" and f2() == "s")
-- a dumped function loaded again gets upvalues of its own
local g = load(string.dump(mk()))
base = count()
g = nil
collectgarbage()
note("loaded", base)
-- a thread freed with open upvalues: the held ones close, the others go
local keep
local co = coroutine.create(function()
  local p, q = {}, {}
  keep = function() return p end
  do local h = function() return q end end
  coroutine.yield()
end)
coroutine.resume(co)
base = count()
co = nil
collectgarbage()
note("thread", base)
keep = nil
collectgarbage()
note("kept closure", base)
print(table.concat(out, "\n"))
