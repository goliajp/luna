-- v2.13 CORPUS-IV: pattern sets, ranges, complements, magic
-- chars inside sets.
print(("hello-world"):match("[a-z]+"))
print(("hello-world"):gsub("[^a-z]", "_"))
print(("a.b[c"):match("[.%[]+", 2))
print(("x-y"):match("[%-]"))
print(("5%"):match("[%%]"))
print(("abc123"):match("[%a%d]+"))
print(("]x"):match("[]]"))
