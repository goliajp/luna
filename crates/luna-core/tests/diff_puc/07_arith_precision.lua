-- v2.10 CORPUS: floating-point precision + int/float promotion.
-- Uses string.format with explicit precision to avoid PUC-vs-luna
-- default float-formatting drift.
print(string.format("%.15f", 0.1 + 0.2))
print(string.format("%.6e", 1e-10))
print(string.format("%.6f", math.pi))
print(math.huge > 0)                  -- true
print(math.huge - math.huge ~= math.huge - math.huge)  -- nan-comparison
print(-math.huge < 0)                 -- true
print(math.tointeger(1.0))
print(math.tointeger(1.5))    -- nil
print(math.tointeger("42"))
print(math.tointeger("3.14")) -- nil
print(string.format("%.10f", math.sqrt(2)))
print(string.format("%.10f", 2^0.5))
