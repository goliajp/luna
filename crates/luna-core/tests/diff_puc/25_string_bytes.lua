-- v2.10 CORPUS: string byte/char manipulation.
print(string.byte("A", 1))
print(string.byte("hello", 1, 5))  -- multiple returns
print(string.char(72, 101, 108, 108, 111))
print(string.char(0x1F600 & 0xff))  -- byte-mask

-- concat multi-type
local n = 42
print("value is " .. n)
print("pi ~ " .. string.format("%.4f", math.pi))
