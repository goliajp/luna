-- v2.13 CORPUS-IV: select with negative index (5.2+) picks from
-- the end; 0 / out-of-range errors.
print(select(-1, "a", "b", "c"))
print(select(-2, "a", "b", "c"))
print(select(-3, "a", "b", "c"))
print((pcall(select, -4, "a", "b", "c")))
print((pcall(select, 0, "a")))
print(select(4, "a", "b", "c"))
print("end")
