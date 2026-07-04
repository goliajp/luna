-- v2.14 HD 5.2 seed: string.rep has NO separator parameter yet
-- (5.4 added it) — a third argument is ignored.
print(string.rep("ab", 3))
print(string.rep("a", 3, ","))
print(string.rep("x", 0) == "")
