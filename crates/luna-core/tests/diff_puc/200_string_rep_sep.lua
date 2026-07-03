-- v2.13 CORPUS-IV: string.rep with separator + zero/one counts.
print(string.rep("ab", 3))
print(string.rep("x", 4, "-"))
print(string.rep("q", 1, "-"))
print(string.rep("q", 0, "-") == "")
print(string.rep("", 5, ","))
print(#string.rep("a", 100))
