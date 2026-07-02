-- v2.11 CORPUS-II: string.format type conversions.
print(string.format("%d", 42))
print(string.format("%d", 42.0))    -- float with exact int repr is OK
print((pcall(string.format, "%d", 42.7)))  -- 5.3+: errors, no int repr
print(string.format("%d", -3))
print(string.format("%s", 42))       -- number → string
print(string.format("%s", nil))
print(string.format("%s", true))
print(string.format("%c", 65))       -- char code
