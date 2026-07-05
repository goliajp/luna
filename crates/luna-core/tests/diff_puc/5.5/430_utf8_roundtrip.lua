-- v2.15 P2.4 utf8: char/codepoint round-trip.
for _, cp in ipairs({0x41, 0x80, 0xff, 0x100, 0x7ff, 0x800, 0xffff, 0x10000, 0x10ffff}) do
  local s = utf8.char(cp)
  local back = utf8.codepoint(s, 1)
  print(cp == back, cp, back)
end
