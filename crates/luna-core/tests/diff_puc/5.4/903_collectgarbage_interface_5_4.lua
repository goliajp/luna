-- v3.1 W3: collectgarbage's interface per dialect — which options exist,
-- what each returns, and how a written pacing parameter reads back (PUC
-- stores them in lossy, version-specific encodings). Values that depend on
-- the collector's progress or memory use are reduced to their shape.
-- 5.2 names a C-called function by walking the global table, whose hash
-- seed changes per run ('_G.collectgarbage' on some runs, 'collectgarbage'
-- on others); drop the prefix so the output is stable.
local function clean(s) return (tostring(s):gsub("'_G%.", "'")) end
local function show(...)
  local n = select("#", ...)
  local t = {}
  for i = 1, n do t[i] = clean((select(i, ...))) end
  return n .. ":" .. table.concat(t, " ")
end
local function try(...) return show(pcall(collectgarbage, ...)) end
local function shape(ok, ...)
  if not ok then return "err " .. clean((...)) end
  local t = {}
  for i = 1, select("#", ...) do t[i] = type((select(i, ...))) end
  return select("#", ...) .. ":" .. table.concat(t, ",")
end
local opts = {"stop", "restart", "collect", "isrunning", "setpause",
  "setstepmul", "setmajorinc", "setstepsize", "incremental", "generational",
  "param", "bogus", 3}
for _, o in ipairs(opts) do
  local ok, err = pcall(collectgarbage, o)
  print(tostring(o), ok, ok and "" or clean(err))
  collectgarbage("restart")
  pcall(collectgarbage, "incremental")
end
print("count", shape(pcall(collectgarbage, "count")))
print("count arg", shape(pcall(collectgarbage, "count", "x")))
print("step arg", shape(pcall(collectgarbage, "step", "x")))
for _, v in ipairs{150, 2000, 10, -8} do
  print("setpause", v, try("setpause", v), try("setpause", 200))
  print("setstepmul", v, try("setstepmul", v), try("setstepmul", 200))
end
print("setpause none", try("setpause"), try("setpause", 200))
if _VERSION == "Lua 5.4" then
  print("inc", try("incremental", 300, 400, 20), try("setpause", 200), try("setstepmul", 100))
  print("gen", try("generational", 30, 200), try("incremental"))
end
if _VERSION == "Lua 5.5" then
  for _, p in ipairs{"minormul", "majorminor", "minormajor", "pause", "stepmul", "stepsize"} do
    print("param", p, try("param", p))
  end
  for _, v in ipairs{1, 99, 101, 123, 128, 12345, 1000000, -1} do
    print("param pause", v, try("param", "pause", v), try("param", "pause"))
  end
  print("param bad", try("param", "bogus"), try("param", 3), try("param"))
end
-- a table's __gc runs from 5.2 on; 5.1 finalizes userdata only
local ran = {}
setmetatable({}, {__gc = function()
  ran[#ran + 1] = "isrunning=" .. tostring(select(2, pcall(collectgarbage, "isrunning")))
end})
collectgarbage()
collectgarbage()
print("table __gc", #ran, ran[1])
