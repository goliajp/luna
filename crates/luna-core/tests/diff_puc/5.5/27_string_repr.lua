-- v2.10 CORPUS: string library conversion.
print(tostring(42))
print(tostring(nil))
print(tostring(true))
print(tostring(false))
print(tonumber("42"))
print(tonumber("42.5"))
print(tonumber("hello"))  -- nil
print(tonumber("0x1a"))   -- 26
print(tonumber("1e3"))    -- 1000
print(tonumber("42", 8))  -- 34 (octal)
print(tonumber("ff", 16)) -- 255
