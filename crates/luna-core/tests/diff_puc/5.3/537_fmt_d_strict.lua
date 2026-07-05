-- v2.14 HD 5.3 seed: %d requires an integer representation.
print(string.format("%d", 42), string.format("%d", 42.0))
print((pcall(string.format, "%d", 42.5)))
print(string.format("%.1f", 1 / 4))
