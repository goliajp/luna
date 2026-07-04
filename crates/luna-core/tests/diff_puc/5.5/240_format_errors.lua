-- v2.13 CORPUS-IV: string.format arity/type failures (flags
-- only — wording carries positions).
print((pcall(string.format, "%d")))
print((pcall(string.format, "%d", "abc")))
print((pcall(string.format, "%y", 1)))
print((pcall(string.format, "%s")))
print(string.format("%s %s", 1, "two"))
print((pcall(string.format, "%d", {})))
