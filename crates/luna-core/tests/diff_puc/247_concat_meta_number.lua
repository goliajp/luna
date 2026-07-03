-- v2.13 CORPUS-IV: concat with numbers needs no metamethod;
-- boolean/nil concat errors; float spelling inside concat.
print(1 .. 2 .. 3)
print(1.5 .. "|" .. 2.0)
print((pcall(function() return "x" .. nil end)))
print((pcall(function() return "x" .. true end)))
print((pcall(function() return {} .. "x" end)))
print(0.1 .. "")
