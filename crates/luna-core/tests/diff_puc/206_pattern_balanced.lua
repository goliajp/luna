-- v2.13 CORPUS-IV: %b balanced match.
print(string.match("(a(b)c)", "%b()"))
print(string.match("f(x, g(y))", "%b()"))
print(string.match("[a[b]c]", "%b[]"))
print(string.match("no parens", "%b()"))
print(string.gsub("f(1) g(2)", "%b()", "()"))
