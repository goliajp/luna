-- v2.11 CORPUS-II: chained string ops.
print(("hello"):upper():sub(1, 3))
print(("  padded  "):match("^%s*(.-)%s*$"))
print(("a,b,c,d"):gsub(",", "-"))
-- method syntax on literals
local s = "hello"
print(s:len(), s:upper(), s:reverse())
