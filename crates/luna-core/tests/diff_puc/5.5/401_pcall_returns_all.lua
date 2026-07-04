-- v2.14 CV.3: pcall forwards every return value.
print(pcall(function() return 1, nil, "three" end))
print(pcall(function() end))
print(pcall(function(...) return select("#", ...) end, 1, 2, 3))
print(xpcall(function(a) return a * 2 end, print, 21))
