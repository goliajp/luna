-- v2.13 CORPUS-IV: tostring type prefixes (address stripped).
print(tostring(print):match("^function: ") ~= nil)
print(tostring({}):match("^table: ") ~= nil)
print(tostring(coroutine.create(function() end)):match("^thread: ") ~= nil)
local f1, f2 = print, print
print(tostring(f1) == tostring(f2))
print(tostring({}) ~= tostring({}))
