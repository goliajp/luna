-- v2.11 CORPUS-II: string.format float precision.
print(string.format("%f", 3.14))
print(string.format("%.2f", 3.14))
print(string.format("%10.4f", 3.14))
print(string.format("%-10.4f|", 3.14))
print(string.format("%e", 1234.5))
print(string.format("%.2e", 1234.5))
print(string.format("%g", 1234.5))
print(string.format("%.3g", 1234.5))
print(string.format("%.0f", 3.7))     -- rounds to 4
