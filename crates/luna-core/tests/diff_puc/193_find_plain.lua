-- v2.13 CORPUS-IV: string.find plain=true treats magic chars
-- literally; pattern mode interprets them.
print(string.find("a.b", ".", 1, true))
print(string.find("a.b", "."))
print(string.find("x%dx", "%d", 1, true))
print(string.find("x5x", "%d"))
print(string.find("hello", "l+"))
print(string.find("hello", "l+", 4))
print(string.find("abc", "^b"))
print(string.find("abc", "^a"))
