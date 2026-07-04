-- v2.14 CV.3: find with captures, plain mode, init offsets.
print(string.find("hello world", "(o)%s(w)"))
print(string.find("a.b", ".", 1, true))
print(string.find("abcabc", "bc", 4))
print(string.find("x", "y"))
print(("abc"):find("b", -2))
