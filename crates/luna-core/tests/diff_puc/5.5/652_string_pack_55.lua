-- v3.1 string slice: string.pack family in 5.5 (size_t sizes, "result too long")
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
P("pack huge c", function() return S.pack("i4c9223372036854775807", 1, "") end)
P("pack size", function() return S.pack("i99999999999999", 1) end)
P("packsize big", function() return S.packsize("c2147483639c9") end)
P("packsize huge", function() return S.packsize("c9223372036854775800c9") end)
P("unpack pos 0", function() return S.unpack("b", "\1\2", 0) end)
P("unpack z open", function() return S.unpack("z", "abc") end)
