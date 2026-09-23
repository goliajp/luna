-- v3.1 string slice: 5.5 argument checks and pattern/gsub semantics (lstrlib 5.5.1)
local function show(...)
  local t = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    if type(v) == "string" then
      v = string.format("%q", (v:gsub("^[^\n]-:%d+: ", ""))):gsub("\n", "n")
    end
    t[#t + 1] = tostring(v)
  end
  return table.concat(t, " ")
end
local function P(label, f, ...)
  print(label, show(pcall(f, ...)))
end
local S = string
P("len none", function() return S.len() end)
P("byte float", function() return S.byte("abc", 1.5) end)
P("char none", function() return S.char(nil) end)
P("rep sep num", function() return S.rep("ab", 2, 0.5) end)
P("find nospecial", function() return S.find("a)b]c", ")b]") end)
P("class end", function() return S.find("a", "[%") end)
P("backref position", function() return S.find("aa", "()a%1") end)
P("gsub bad repl", function() return S.gsub("abc", "b", true) end)
P("gsub bad repl none", function() return S.gsub("abc", "b") end)
P("gsub open capture", function() return S.gsub("abc", "(a", "x") end)
P("gsub pos capture", function() return S.gsub("abc", "()b", "[%1]") end)
P("gmatch init", function()
  local r = {}
  for w in S.gmatch("abcd", ".", -2) do r[#r + 1] = w end
  return table.concat(r)
end)
P("gmatch caret", function()
  local r = {}
  for w in S.gmatch("^a^a", "^a") do r[#r + 1] = w end
  return table.concat(r, ",")
end)
P("gmatch done", function() local it = S.gmatch("a", "a") it() return select("#", it()) end)
P("dump native", function() return S.dump(S.len) end)
P("dump string", function() return S.dump("x") end)
P("dump none", function() return S.dump() end)
P("s name", function() return (tostring(setmetatable({}, {__name = "MyType"})):gsub("0x%x+", "ADDR")) end)
