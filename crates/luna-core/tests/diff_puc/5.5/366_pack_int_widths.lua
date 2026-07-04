-- v2.14 CV.3: fixed-width signed/unsigned little-endian ints.
local function hex(s)
  return (s:gsub(".", function(c) return string.format("%02X", c:byte()) end))
end
print(hex(string.pack("<b", -1)), hex(string.pack("<B", 255)))
print(hex(string.pack("<h", -2)), hex(string.pack("<H", 0xBEEF)))
print(hex(string.pack("<i4", -1)), hex(string.pack("<I4", 0xDEADBEEF)))
print(hex(string.pack("<i8", 258)))
print(string.unpack("<b", "\xFF"))
print(string.unpack("<H", "\xEF\xBE"))
print(string.unpack("<i8", string.pack("<i8", -123456789)))
