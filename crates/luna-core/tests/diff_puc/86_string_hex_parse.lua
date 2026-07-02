-- v2.11 CORPUS-II: hex + scientific literals.
print(tonumber("0xff"))
print(tonumber("0Xff"))
print(tonumber("0x1p8"))     -- 256.0 (hex-float)
print(tonumber("1e3"))
print(tonumber("1.5e2"))
print(tonumber(" 42 "))       -- allowed with whitespace
print(tonumber("42abc"))      -- nil (junk suffix)
print(tonumber(""))           -- nil
