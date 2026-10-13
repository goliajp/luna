-- rule: lcode.fixjump.1 lcode.luaK_checkstack.1 lcode.luaK_concat.1 lparser.errorlimit.1 lparser.new_localvar.1
-- Errors the code generator raises at its limits (registers, jump
-- offsets, locals, upvalues), in many contexts: the message, the line and
-- the token PUC's scanner is on (`near`). Sources are built here and
-- loaded; the chunk name is "=c".
local load = loadstring or load
local rep, cat = string.rep, table.concat
local function list(n, f, sep)
  local t = {}
  for i = 1, n do t[i] = f(i) end
  return cat(t, sep or ", ")
end
local function try(name, src)
  local f, e = load(src, "=c")
  print(name, f and "ok" or e)
end

-- the last free register runs out at each step of each context: 190
-- locals, then an expression taking n registers
local fillers = {
  call = function(n) return "f(" .. list(n, function() return "1" end) .. ")" end,
  cat = function(n) return list(n, function(i) return "s" .. i end, " .. ") end,
  meth = function(n) return "o:m(" .. list(n, function() return "1" end) .. ")" end,
  nl = function(n) return "f(\n" .. list(n, function() return "1" end, ",\n") .. "\n)" end,
}
local contexts = {
  "local x = X", "local x, y = X", "local x, y, z = 1, X", "x = X", "x, y = X, 1",
  "x, y = 1, X", "t.a = X", "t[k] = X", "t[X] = 1", "return X", "return 1, X",
  "return X, 1", "f(X)", "f(1, X)", "f(X, 1)", "t = {X}", "t = {X, 1}", "t = {1, X}",
  "t = {k = X}", "t = {[X] = 1}", "t = {[1] = X, 2}", "if X then end",
  "if a then elseif X then end", "while X do end", "repeat until X",
  "for i = X, 2 do end", "for i = 1, X do end", "for i = 1, 2, X do end",
  "for k in X do end", "for k, v in X, 1 do end", "x = X + 1", "x = 1 + X",
  "x = a + X", "x = X == 1", "x = 1 < X", "x = -X", "x = not X", "x = #X",
  "x = (X)", "x = X .. 'a'", "x = 'a' .. X", "x = a and X", "x = X or b",
  "x = X.y", "x = X[1]", "x = X:m()", "local function q() return X end",
  "x = function() return X end", "do local y = X end",
}
local fnames = { "call", "cat", "meth", "nl" }
for ci, ctx in ipairs(contexts) do
  for _, fname in ipairs(fnames) do
    for n = 55, 68 do
      local x = fillers[fname](n)
      try(ci .. ":" .. fname .. ":" .. n, "local " .. list(190, function(i) return "a" .. i end) ..
        "\n" .. (ctx:gsub("X", function() return x end)))
    end
  end
end

-- a limit met before a syntax error: PUC generates code as it parses, so
-- the limit error is reported
local pre = "local " .. list(190, function(i) return "a" .. i end) .. "\n"
local function idx(n) return "t[f(" .. list(n, function() return "1" end) .. ")]" end
for n = 45, 70 do
  try("before.stat." .. n, pre .. idx(n))
  try("before.ctor." .. n, pre .. "x = {" .. fillers.cat(n) .. "}[1]")
  try("before.late." .. n, pre .. "x = " .. fillers.call(n) .. "\nlocal = 1")
  try("before.open." .. n, pre .. "if a then\nwhile b do\nx = " .. fillers.nl(n))
  try("before.scan." .. n, pre .. "x = " .. fillers.call(n) .. " .. 'abc")
end

-- locals, parameters and upvalues
local function names(n, p) return list(n, function(i) return (p or "a") .. i end) end
try("locals201", "local " .. names(201))
try("locals201s", list(201, function(i) return "local a" .. i end, "\n"))
try("locals201f", "function f()\n" .. list(201, function(i) return "local a" .. i end, "\n") .. "\nend")
try("forlocals", "local " .. names(197) .. "\nfor i = 1, 2 do end")
try("forinlocals", "local " .. names(195) .. "\nfor k, v, w in f do end")
try("forinlocals2", "local " .. names(196) .. "\nfor k, v, w in f do end")
try("params", "function f(" .. names(260) .. ") end")
try("upvals", list(150, function(i) return "local u" .. i end, "\n") .. "\nfunction g()\n" ..
  list(150, function(i) return "local w" .. i end, "\n") .. "\nfunction f() return " ..
  names(150, "u") .. ",\n" .. names(150, "w") .. " end end")
try("ret300", "return " .. list(300, function() return "1" end))
try("retv255", "return " .. names(255, "v"))

-- jumps past the limit of their offset: bodies of more than 131071
-- instructions, and and/or chains whose first jump goes past them
local body = rep("x = y\n", 70000)
local jumps = {
  {"if", "if c then\n" .. body .. "end\nz = 1"},
  {"ifeof", "if c then\n" .. body .. "end"},
  {"ifelse", "if c then\n" .. body .. "else z = 1 end"},
  {"while", "while c do\n" .. body .. "end"},
  {"repeat", "repeat\n" .. body .. "until c\nz = 1"},
  {"for", "for i = 1, 2 do\n" .. body .. "end"},
  {"forin", "for k, v in pairs(t) do\n" .. body .. "end"},
  {"break", "while c do\nbreak\n" .. body .. "end"},
  {"fn", "function f()\n" .. body .. "end"},
}
if _VERSION ~= "Lua 5.1" then
  jumps[#jumps + 1] = {"goto", "::top::\n" .. body .. "goto top"}
  jumps[#jumps + 1] = {"gotofwd", "goto done\n" .. body .. "::done::"}
end
for _, j in ipairs(jumps) do try("body." .. j[1], j[2]) end
-- 5.1 to 5.3: the chain fails at the same length (`luaK_concat` patches
-- every jump to the end of the chain)
if _VERSION == "Lua 5.1" or _VERSION == "Lua 5.2" or _VERSION == "Lua 5.3" then
  local function chain(n, op, sep) return "a" .. rep(sep .. op .. " a", n - 1) end
  for _, n in ipairs({ 43692, 43693 }) do
    try("and.local." .. n, "local x = " .. chain(n, "and", " "))
    try("or.local." .. n, "local x = " .. chain(n, "or", " ") .. "\nfoo()")
    try("and.nl." .. n, "local x = " .. chain(n, "and", "\n") .. "\n\nlocal y")
  end
  for _, n in ipairs({ 43691, 43692 }) do
    try("and.if." .. n, "if " .. chain(n, "and", " ") .. " then x = 1 end y = 2")
  end
  for _, n in ipairs({ 43690, 43691 }) do
    try("and.while." .. n, "while " .. chain(n, "and", " ") .. " do x = 1 end")
  end
end

-- the same errors from a source read through a reader function
local function pieces(s)
  local i = 0
  return function()
    i = i + 1
    return s:sub((i - 1) * 1000 + 1, i * 1000)
  end
end
local function tryr(name, src)
  local f, e = (load == loadstring and _G.load or load)(pieces(src), "=c")
  print("reader " .. name, f and "ok" or e)
end
tryr("args", "local " .. list(190, function(i) return "a" .. i end) .. "\nf(\n" ..
  list(70, function() return "1" end, ",\n") .. "\n)")
tryr("while", "while c do\n" .. body .. "end")
tryr("before", pre .. "f(\n" .. list(70, function() return "1" end, ",\n") .. "\n) )")
