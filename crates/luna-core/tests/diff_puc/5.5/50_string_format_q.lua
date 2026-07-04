-- v2.11 CORPUS-II: string.format %q roundtrip.
print(string.format("%q", "simple"))
print(string.format("%q", 'with "quotes"'))
print(string.format("%q", "tab\there\nline"))
print(string.format("%q", true))
print(string.format("%q", nil))
print(string.format("%q", 42))
