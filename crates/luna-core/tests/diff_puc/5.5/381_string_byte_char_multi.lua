-- v2.14 CV.3: byte/char multi-value legs + negative indices.
print(string.byte("ABC", 1, 3))
print(string.byte("ABC", -1))
print(string.char(72, 105, 33))
print(("XYZ"):byte(2))
print(string.byte("A", 5))
