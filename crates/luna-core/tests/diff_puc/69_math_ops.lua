-- v2.11 CORPUS-II: math library. Restored math.modf now that
-- v2.12 KNOWN-DIV fix aligns integer-return with PUC 5.5.
-- Float precision on math.log and math.modf fraction guarded
-- via string.format to survive PUC-vs-luna default precision.
print(math.floor(3.7))
print(math.floor(-3.2))
print(math.ceil(3.2))
print(math.ceil(-3.7))
local ip, fp = math.modf(3.7)
print(ip, string.format("%.6f", fp))
local ip2, fp2 = math.modf(-3.7)
print(ip2, string.format("%.6f", fp2))
print(math.fmod(10, 3))
print(math.exp(0))
print(string.format("%.6f", math.log(math.exp(2))))
print(string.format("%.6f", math.log(100, 10)))
