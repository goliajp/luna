-- v2.13 CORPUS-IV: %s precision truncation + %% literal + %c.
print(string.format("%.3s", "hello"))
print(string.format("%10s|", "abc"))
print(string.format("%-10s|", "abc"))
print(string.format("100%%"))
print(string.format("%c%c%c", 97, 98, 99))
print(string.format("%.0s", "gone") .. "|")
