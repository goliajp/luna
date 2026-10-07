-- Which C calls leave errno behind, and which failures then show it: each
-- setter, then each failure. File names are under the global DIR when it
-- is set.
local D = DIR or ""
local function P(n) return D .. n end
io.stdout:setvbuf("no")
local function esc(s)
  if s == nil then return "nil" end
  if io.type(s) then return "file" end
  if type(s) ~= "string" then return tostring(s) end
  return (s:gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end))
end
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = esc((select(i, ...))) end return table.concat(t, ",") end
local function put(name, s) local f = assert(io.open(name, "wb")); f:write(s); f:close() end
local SETTERS = {
  { "none", function() end },
  { "strtod", function() return tonumber("1e999") end },
  { "strtod-under", function() return tonumber("1e-999") end },
  { "lexer", function() return (loadstring or load)("return 1e999")() end },
  { "log0", function() return math.log(0) end },
  { "logneg", function() return math.log(-1) end },
  { "sqrtneg", function() return math.sqrt(-1) end },
  { "fmod0", function() return math.fmod(1, 0.0) end },
  { "acos2", function() return math.acos(2) end },
  { "exp", function() return math.exp(1000) end },
  { "pow", function() return 10 ^ 400 end },
  { "open", function() return io.open(P("no/such/file")) end },
  { "remove", function() return os.remove(P("no-such-file")) end },
  { "require", function() return pcall(require, "no_such_module") end },
  { "date", function() return os.date("%Y", 0) end },
  { "time", function() return os.time({ year = 2000, month = 1, day = 1 }) end },
  { "tmpname", function() local n = os.tmpname(); os.remove(n) end },
  { "rename", function() return os.rename(P("no-such-a"), P("no-such-b")) end },
  { "readnum", function() local f = io.open(P("n.txt"), "w"); f:write("1e999"); f:close()
      f = io.open(P("n.txt"), "r"); local x = f:read("*n"); f:close(); return x end },
  { "seekfail", function() local f = io.open(P("n.txt"), "r"); f:seek("set", -1); f:close() end },
  { "date-bad", function() return pcall(os.date, "%Ez") end },
  { "execute", function() return os.execute("exit 2") end },
}
local FAILS = {
  { "write-after-read", function()
      put(P("rw.txt"), "aa\nbb\n"); local f = io.open(P("rw.txt"), "r+"); f:read("*l")
      local a = r(f:write("X")); f:close(); return a end },
  { "read-after-write", function()
      put(P("rw.txt"), "aa\nbb\n"); local f = io.open(P("rw.txt"), "r+"); f:read("*l"); f:seek("cur", 0)
      f:write("X"); local a = r(f:read("*l")); f:close(); return a end },
  { "write-readonly", function()
      put(P("ro.txt"), "aa\n"); local f = io.open(P("ro.txt"), "r"); local a = r(f:write("X")); f:close(); return a end },
  { "read-writeonly", function()
      local f = io.open(P("wo.txt"), "w"); local a = r(f:read("*l")); f:close(); return a end },
  { "seek-neg", function()
      put(P("ro.txt"), "aa\n"); local f = io.open(P("ro.txt"), "r"); local a = r(f:seek("set", -1)); f:close(); return a end },
  { "execute", function() return r(os.execute("exit 3")) end },
  { "pclose", function() local p = io.popen("exit 3"); return r(p:close()) end },
}
for _, s in ipairs(SETTERS) do
  local t = {}
  for _, f in ipairs(FAILS) do
    s[2]()
    t[#t+1] = f[1] .. "=" .. f[2]()
  end
  print(s[1], table.concat(t, " | "))
end
