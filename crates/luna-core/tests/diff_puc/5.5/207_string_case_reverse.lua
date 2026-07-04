-- v2.13 CORPUS-IV: reverse/upper/lower on edges.
print(string.reverse("abc"), string.reverse(""), string.reverse("x"))
print(string.upper("MiXeD 123 !"), string.lower("MiXeD 123 !"))
print(("ß"):upper() == "ß")
print(#string.reverse("ab\0cd"))
print(string.upper(""))
