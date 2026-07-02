-- v2.12 CORPUS-III: string.format padding + precision combos.
print(string.format("[%5.2f]", 3.14))
print(string.format("[%-5.2f]", 3.14))
print(string.format("[%08.2f]", 3.14))
print(string.format("[%+8.2f]", 3.14))
print(string.format("[%+-8.2f]", 3.14))
-- %s with precision limits length
print(string.format("[%.5s]", "hellothere"))
print(string.format("[%10.5s]", "hellothere"))
