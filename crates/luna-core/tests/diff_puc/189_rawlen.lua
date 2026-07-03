-- v2.13 CORPUS-IV: rawlen on tables and strings; rejects others.
print(rawlen({ 1, 2, 3 }))
print(rawlen(""))
print(rawlen("hello"))
print(rawlen(setmetatable({ 1, 2 }, { __len = function() return 99 end })))
print((pcall(rawlen, 42)))
print((pcall(rawlen, nil)))
