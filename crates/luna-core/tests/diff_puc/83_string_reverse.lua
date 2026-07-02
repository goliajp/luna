-- v2.11 CORPUS-II: string.reverse byte-order (not char).
print(string.reverse(""))
print(string.reverse("a"))
print(string.reverse("hello"))
print(string.reverse("12345"))
-- reversing byte-wise (not unicode)
print(#string.reverse("abc"))
