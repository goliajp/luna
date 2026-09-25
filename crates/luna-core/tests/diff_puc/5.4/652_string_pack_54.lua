-- v3.1 string slice: string.pack family in 5.4
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
local function hex(s) return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end)) end
P("pack missing", function() return S.pack("i4i4", 1) end)
P("pack missing str", function() return S.pack("z") end)
P("pack fmt zero", function() return hex(S.pack("i1\0i1", 1, 2)) end)
P("pack num str", function() return hex(S.pack("<d", "1.5")) end)
P("pack option", function() return S.pack("q", 1) end)
P("packsize big", function() return S.packsize("c2147483639c9") end)
P("unpack pos 0", function() return S.unpack("b", "\1\2", 0) end)
P("unpack pos neg", function() return S.unpack("b", "\1\2", -3) end)
P("unpack z open", function() return S.unpack("z", "abc") end)
P("unpack i16", function() return S.unpack("<i16", S.rep("\255", 16)) end)
