-- v2.13 CORPUS-IV: string.format %g/%e/%f precision matrix.
print(string.format("%g", 100000), string.format("%g", 10000000))
print(string.format("%g", 0.0001), string.format("%g", 0.00001))
print(string.format("%.3g", 3.14159), string.format("%.10g", 3.14159))
print(string.format("%e", 12345.678))
print(string.format("%.2e", 12345.678))
print(string.format("%f", 1/3), string.format("%.1f", 2.55))
print(string.format("%g", 2^53))
