-- v2.14 CV.3: malformed format strings via pcall shape.
print(pcall(string.pack, "<q", 1))
print(pcall(string.pack, "s9", "x"))
print(pcall(string.unpack, "<s4", "\255\255\255\255"))
