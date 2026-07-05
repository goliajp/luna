-- v2.14 CV.3: endianness prefixes.
local function hex(s)
  return (s:gsub(".", function(c) return string.format("%02X", c:byte()) end))
end
print(hex(string.pack(">i4", 1)), hex(string.pack("<i4", 1)))
print(hex(string.pack(">h", 0x0102)), hex(string.pack("<h", 0x0102)))
print(string.unpack(">i4", "\0\0\1\0"))
print(string.unpack("<i4", "\0\0\1\0"))
