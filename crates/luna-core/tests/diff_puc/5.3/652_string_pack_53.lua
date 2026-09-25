-- v3.1 string slice: string.pack family in 5.3
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
P("pack X end", function() return S.pack("iX", 1) end)
P("pack X char", function() return S.pack("Xc1") end)
P("pack align", function() return S.pack("!3 i4", 1) end)
P("pack size", function() return S.pack("i17", 1) end)
P("pack c size", function() return S.pack("c", "a") end)
P("pack option", function() return S.pack("q", 1) end)
P("packsize s", function() return S.packsize("s") end)
P("packsize big", function() return S.packsize("c2147483639c9") end)
P("unpack pos 0", function() return S.unpack("b", "\1\2", 0) end)
P("unpack pos neg", function() return S.unpack("b", "\1\2", -3) end)
P("unpack z open", function() return S.unpack("z", "abc") end)
P("unpack z open 2", function() return S.unpack("zb", "abc") end)
P("unpack float str", function() return S.unpack("<f", S.pack("<f", 0.5)) end)
P("unpack i16", function() return S.unpack("<i16", S.rep("\255", 16)) end)
P("unpack i9 bad", function() return S.unpack("<I9", S.rep("\255", 9)) end)
P("unpack s short", function() return S.unpack("s1", "\5ab") end)
