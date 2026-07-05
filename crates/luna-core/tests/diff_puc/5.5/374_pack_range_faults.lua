-- v2.14 CV.3: overflow + truncated-data errors via pcall shape.
print(pcall(string.pack, "<b", 200))
print(pcall(string.pack, "<h", 70000))
print(pcall(string.unpack, "<i4", "ab"))
print(pcall(string.pack, "<i17", 1))
