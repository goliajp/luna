-- v3.1 string slice: 5.1 argument conversions and error wording (lstrlib 5.1.5)
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
-- integer arguments are C casts of the number: truncation, no errors
P("sub float", function() return S.sub("abcdef", 2.7, 4.2) end)
P("byte float", function() return S.byte("abcdef", 2.5) end)
P("char trunc", function() return S.char(65.9, 66.1) end)
P("char range", function() return S.char(256) end)
P("rep float", function() return S.rep("ab", 2.9) end)
-- no separator argument in 5.1
P("rep sep", function() return S.rep("ab", 3, ",") end)
-- missing argument vs explicit nil
P("len none", function() return S.len() end)
P("sub nil", function() return S.sub("abc", nil) end)
-- numbers become strings with %.14g
P("len float", function() return S.len(0.1), S.upper(1e15), S.reverse(2^53) end)
-- a start past the end is clamped, not rejected
P("find past end", function() return S.find("abc", "", 10) end)
P("find past end pat", function() return S.find("abc", "()", 10) end)
-- patterns are C strings: a zero byte ends them
P("pattern zero", function() return S.find("a\0b", "%w\0x") end)
P("gsub pattern zero", function() return S.gsub("aXb", "X\0", "-") end)
-- %g is not a class in 5.1
P("no %g", function() return S.find("a g", "%g") end)
-- capture-index errors carry no index; %b errors read differently
P("capture index", function() return S.find("a", "%1") end)
P("gsub capture index", function() return S.gsub("abc", "b", "%2") end)
P("balance", function() return S.find("a", "%b") end)
-- a bad escape in a replacement is copied through
P("gsub escape", function() return S.gsub("abc", "b", "[%x]") end)
-- empty matches: taken, then one byte copied
P("gsub empty", function() return S.gsub("abc", "%w*", "-") end)
P("gsub empty 2", function() return S.gsub("a,,b", ",*", "|") end)
P("gmatch empty", function()
  local r = {}
  for w in S.gmatch("abc", "%w*") do r[#r + 1] = "<" .. w .. ">" end
  return table.concat(r)
end)
-- gmatch: '^' is an ordinary character
P("gmatch caret", function()
  local r = {}
  for w in S.gmatch("^a^a", "^a") do r[#r + 1] = w end
  return table.concat(r, ",")
end)
-- the replacement count is a C int
P("gsub max float", function() return S.gsub("aaaa", "a", "b", 2.7) end)
P("gsub max neg", function() return S.gsub("aaaa", "a", "b", -1) end)
-- gsub with an open capture and a plain template succeeds
P("gsub open capture", function() return S.gsub("abc", "(a", "x") end)
-- string.byte on a 5.1 C stack
P("byte many", function() return select("#", S.byte(S.rep("a", 7997), 1, -1)) end)
P("byte too many", function() return select("#", S.byte(S.rep("a", 7998), 1, -1)) end)
print(utf8, string.pack, string.gfind == string.gmatch)
