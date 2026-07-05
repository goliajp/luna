-- v2.10 CORPUS: integer/float distinction (Lua 5.3+ integer subtype).
print(math.type(1))
print(math.type(1.0))
print(math.type(1 // 1))
print(math.type(1 / 1))    -- float (division always float)
print(math.type(2^0))       -- float (^ always float)
print(math.type(1 + 1))
print(math.type(1 + 1.0))  -- float (contagion)
print(math.type(1 << 1))   -- integer

-- string to integer
print(math.type(tonumber("10")))
print(math.type(tonumber("10.0")))
