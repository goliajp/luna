-- v2.11 CORPUS-II: string.rep with separator.
print(string.rep("ab", 3, "-"))
print(string.rep("x", 5, ","))
print(string.rep("", 5, ","))    -- ",,,,"
print(string.rep("a", 1, "-"))
print(string.rep("a", 0, "-"))   -- empty
