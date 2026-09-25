-- v3.1 string slice: 5.2 argument conversions, pattern and gsub semantics (lstrlib 5.2.4)
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
P("sub float", function() return S.sub("abcdef", 2.7, 4.2) end)
P("char trunc", function() return S.char(65.9) end)
P("char range", function() return S.char(-1) end)
P("rep sep", function() return S.rep("ab", 3, ","), S.rep("", 3, "-") end)
P("rep sep num", function() return S.rep("ab", 2, 0.5) end)
P("len none", function() return S.len() end)
P("len float", function() return S.len(0.1), S.upper(1e15) end)
P("find past end", function() return S.find("abc", "", 10) end)
P("pattern zero", function() return S.find("a\0b", "\0b"), S.find("a\0b", "[\0]") end)
P("find nospecial", function() return S.find("a)b]c", ")b]") end)
P("capture index", function() return S.find("a", "%1") end)
P("gsub capture index", function() return S.gsub("abc", "b", "%2") end)
P("balance", function() return S.find("a", "%b") end)
P("class end", function() return S.find("a", "[%") end)
P("gsub escape", function() return S.gsub("abc", "b", "[%x]") end)
P("gsub empty", function() return S.gsub("abc", "%w*", "-") end)
P("gmatch empty", function()
  local r = {}
  for w in S.gmatch("abc", "%w*") do r[#r + 1] = "<" .. w .. ">" end
  return table.concat(r)
end)
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
-- the count is a size_t: a negative one means no limit
P("gsub max neg", function() return S.gsub("aaaa", "a", "b", -1) end)
P("gsub bad repl", function() return S.gsub("abc", "b", true) end)
P("gsub pos capture", function() return S.gsub("abc", "()b", "[%1]") end)
P("gsub open capture", function() return S.gsub("abc", "(a", "x") end)
P("gsub fn number", function() return S.gsub("ab", "%w", function() return 2.5 end) end)
P("dump strip ignored", function() return type(S.dump(function() end, true)) end)
print(utf8, string.pack)
