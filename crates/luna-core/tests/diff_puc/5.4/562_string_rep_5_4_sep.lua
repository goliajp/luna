-- v2.15 P2.5: string.rep with separator (5.4+ semantics).
print(string.rep("x", 5, "-"))     -- x-x-x-x-x
print(string.rep("ab", 3, "|"))    -- ab|ab|ab
print(string.rep("", 5, "-"))       -- "----"
print(string.rep("x", 1, "-"))      -- x
print(string.rep("x", 0, "-"))      -- (empty)
