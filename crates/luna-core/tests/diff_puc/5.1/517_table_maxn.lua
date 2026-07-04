-- v2.14 HD 5.1 seed: table.maxn (removed in 5.2).
print(table.maxn({ 1, 2, 3 }))
print(table.maxn({ [1] = "a", [5] = "b" }))
print(table.maxn({}))
print(table.maxn({ [2.5] = "frac" }))
