-- v2.11 CORPUS-II: string.char + string.byte edges.
print(string.char(0))
print(string.byte(string.char(0), 1))
print(string.byte("A"))
print(string.byte("A", -1))
print(#string.char(0, 0, 0))  -- 3

-- byte range: 0-255
print(string.byte(string.char(255), 1))
