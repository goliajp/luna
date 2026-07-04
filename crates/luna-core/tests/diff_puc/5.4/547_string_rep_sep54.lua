-- v2.14 HD 5.4 seed: string.rep separator parameter arrives
-- (contrast 5.2/530: third arg ignored there).
print(string.rep("a", 3, ","))
print(string.rep("ab", 2, "--"))
print(string.rep("x", 1, ";"))
print(string.rep("x", 0, ";") == "")
