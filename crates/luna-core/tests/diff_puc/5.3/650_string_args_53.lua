-- v3.1 string slice: 5.3 argument checks and pattern semantics (lstrlib 5.3.6)
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
P("sub float", function() return S.sub("abcdef", 2.5) end)
P("len none", function() return S.len() end)
P("len float", function() return S.len(0.1), S.upper(3.0) end)
P("rep sep num", function() return S.rep("ab", 2, 0.5) end)
P("char range", function() return S.char(256) end)
P("find nospecial", function() return S.find("a)b]c", ")b]") end)
P("class end", function() return S.find("a", "[%") end)
P("backref position", function() return S.find("aa", "()a%1") end)
P("gsub capture index", function() return S.gsub("abc", "b", "%2") end)
P("gsub pos capture", function() return S.gsub("abc", "()b", "[%1]") end)
P("gsub open capture", function() return S.gsub("abc", "(a", "x") end)
P("gsub open capture fn", function() return S.gsub("abc", "(a", S.len) end)
P("gsub bad repl", function() return S.gsub("abc", "b", true) end)
P("gsub empty", function() return S.gsub("abc", "%w*", "-") end)
P("gsub unchanged", function() local s = "ab" .. "c" return rawequal(S.gsub(s, "x", "y"), s) end)
P("gmatch caret", function()
  local r = {}
  for w in S.gmatch("^a^a", "^a") do r[#r + 1] = w end
  return table.concat(r, ",")
end)
P("gmatch init ignored", function()
  local r = {}
  for w in S.gmatch("abc", ".", 3) do r[#r + 1] = w end
  return table.concat(r)
end)
P("gmatch done", function() local it = S.gmatch("a", "a") it() return select("#", it()) end)
P("dump native", function() return S.dump(S.len) end)
P("byte slice", function() return select("#", S.byte(S.rep("a", 9000), 1, -1)) end)
