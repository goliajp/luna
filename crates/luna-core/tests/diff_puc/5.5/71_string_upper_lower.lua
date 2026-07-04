-- v2.11 CORPUS-II: string case conversion.
print(string.upper("hello"))
print(string.lower("HELLO"))
print(string.upper("Hello, World!"))
print(string.lower("Hello, World!"))
-- non-alpha unchanged
print(string.upper("123abc"))
print(string.lower("ABC123"))
