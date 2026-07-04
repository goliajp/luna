-- v2.14 CV.3: select '#' and positive/negative indices.
print(select("#"))
print(select("#", nil, nil))
print(select(2, "a", "b", "c"))
print(select(-1, "x", "y", "z"))
print(select(-2, "x", "y", "z"))
print(select(5, "a"))
