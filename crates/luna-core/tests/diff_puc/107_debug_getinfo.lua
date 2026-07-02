-- v2.12 CORPUS-III: debug.getinfo (source/line/what only).
local info = debug.getinfo(1, "Sl")
print(type(info), info.currentline > 0, info.what)
-- source path is impl-specific, don't compare
